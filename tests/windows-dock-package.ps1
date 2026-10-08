param([Parameter(Mandatory=$true)][string]$Output)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_ENVIRONMENT -ne 'github-hosted' -or $env:PLEAMAR_WM_CI_DOCK_PACKAGE -ne '1') {
    throw 'Package registration is restricted to the explicit disposable GitHub-hosted test.'
}
$out = [IO.Path]::GetFullPath($Output)
$temp = [IO.Path]::GetFullPath($env:RUNNER_TEMP).TrimEnd('\')
if (-not $out.StartsWith($temp + '\', [StringComparison]::OrdinalIgnoreCase) -or (Test-Path -LiteralPath $out)) {
    throw 'Use a fresh evidence directory below RUNNER_TEMP.'
}
New-Item -ItemType Directory -Path $out | Out-Null
$payload = Join-Path $out 'payload'
New-Item -ItemType Directory -Path $payload | Out-Null
$report = @{ passed=$false; locally_installed=$false; disposable_runner=$true; cleanup=@{} }
$certificate = $null
$trusted = $null
$registered = $null
$registrationAttempted = $false
try {
    if (Get-AppxPackage -Name 'Pleamar.NativeDockTest') { throw 'The test package already exists; refusing to replace it.' }
    $sdk = Get-ChildItem -Path "${env:ProgramFiles(x86)}\Windows Kits\10\bin\*\x64\makeappx.exe" -File |
        Sort-Object FullName -Descending | Select-Object -First 1
    if (-not $sdk) { throw 'Windows SDK packaging tools are unavailable.' }
    $sign = Join-Path $sdk.DirectoryName 'signtool.exe'
    Copy-Item -LiteralPath 'target/release/examples/windows-dock-fixture.exe' -Destination (Join-Path $payload 'fixture.exe')
    $monitors = & target/release/pleamar-wm.exe monitors | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) { throw 'Monitor discovery failed.' }
    $screen = @($monitors | Where-Object primary | Select-Object -First 1)
    if ($screen.Count -ne 1) { throw 'Disposable desktop has no primary output.' }
    $config = @{monitor=$screen[0].name;output=$out;disposable_github_runner=$true} | ConvertTo-Json
    [IO.File]::WriteAllText((Join-Path $payload 'dock-fixture.json'),$config,(New-Object Text.UTF8Encoding $false))
    Add-Type -AssemblyName System.Drawing
    foreach ($size in @(44,50,150)) {
        $bitmap = New-Object Drawing.Bitmap($size,$size)
        $graphics = [Drawing.Graphics]::FromImage($bitmap)
        try {
            $graphics.Clear([Drawing.Color]::DarkCyan)
            $bitmap.Save((Join-Path $payload "logo$size.png"),[Drawing.Imaging.ImageFormat]::Png)
        } finally { $graphics.Dispose(); $bitmap.Dispose() }
    }
    @'
<?xml version="1.0" encoding="utf-8"?>
<Package xmlns="http://schemas.microsoft.com/appx/manifest/foundation/windows10"
 xmlns:uap="http://schemas.microsoft.com/appx/manifest/uap/windows10"
 xmlns:uap3="http://schemas.microsoft.com/appx/manifest/uap/windows10/3"
 xmlns:uap10="http://schemas.microsoft.com/appx/manifest/uap/windows10/10"
 xmlns:rescap="http://schemas.microsoft.com/appx/manifest/foundation/windows10/restrictedcapabilities"
 IgnorableNamespaces="uap uap3 uap10 rescap">
 <Identity Name="Pleamar.NativeDockTest" Publisher="CN=Pleamar Native Dock CI" Version="1.0.0.0" ProcessorArchitecture="x64" />
 <Properties><DisplayName>Pleamar owned package</DisplayName><PublisherDisplayName>Pleamar CI</PublisherDisplayName><Logo>logo50.png</Logo></Properties>
 <Resources><Resource Language="en-us" /></Resources>
 <Dependencies><TargetDeviceFamily Name="Windows.Desktop" MinVersion="10.0.19041.0" MaxVersionTested="10.0.22621.0" /></Dependencies>
 <Applications>
  <Application Id="Fixture" Executable="fixture.exe" uap10:RuntimeBehavior="packagedClassicApp" uap10:TrustLevel="mediumIL">
   <uap:VisualElements DisplayName="Pleamar owned package" Description="Disposable dock activation fixture" Square150x150Logo="logo150.png" Square44x44Logo="logo44.png" BackgroundColor="transparent" />
   <Extensions><uap:Extension Category="windows.fileTypeAssociation"><uap3:FileTypeAssociation Name="pleamar-dock-fixture" Parameters="&quot;%1&quot;" MultiSelectModel="Document"><uap:SupportedFileTypes><uap:FileType>.plmdock</uap:FileType></uap:SupportedFileTypes></uap3:FileTypeAssociation></uap:Extension></Extensions>
  </Application>
 </Applications>
 <Capabilities><rescap:Capability Name="runFullTrust" /></Capabilities>
</Package>
'@ | Set-Content -LiteralPath (Join-Path $payload 'AppxManifest.xml') -Encoding UTF8
    $package = Join-Path $out 'owned.msix'
    & $sdk.FullName pack /d $payload /p $package /o
    if ($LASTEXITCODE -ne 0) { throw 'Package validation failed.' }
    $certificate = New-SelfSignedCertificate -Type Custom -Subject 'CN=Pleamar Native Dock CI' -KeyUsage DigitalSignature `
        -FriendlyName 'Disposable pleamar dock fixture' -CertStoreLocation Cert:\CurrentUser\My `
        -TextExtension @('2.5.29.37={text}1.3.6.1.5.5.7.3.3','2.5.29.19={text}')
    $public = Join-Path $out 'fixture.cer'
    Export-Certificate -Cert $certificate -FilePath $public | Out-Null
    # Trust only this generated package signer on this disposable runner.
    $trusted = Import-Certificate -FilePath $public -CertStoreLocation Cert:\LocalMachine\TrustedPeople
    & $sign sign /fd SHA256 /sha1 $certificate.Thumbprint /s My $package
    if ($LASTEXITCODE -ne 0) { throw 'Package signing failed.' }
    # A signed package trusted by this disposable runner uses its existing
    # deployment policy. Do not rewrite machine policy to make a test pass.
    $registrationAttempted = $true
    Add-AppxPackage -Path $package
    $registered = Get-AppxPackage -Name 'Pleamar.NativeDockTest'
    if (-not $registered) { throw 'The test package was not registered.' }
    $env:PLEAMAR_WM_PACKAGE_AUMID = $registered.PackageFamilyName + '!Fixture'
    $env:PLEAMAR_WM_PACKAGE_OUTPUT = $out
    $report.package = $registered.PackageFullName
    $report.aumid = $env:PLEAMAR_WM_PACKAGE_AUMID
    & cargo test --release --locked --bin pleamar-wm windows_backend::dock::package_tests::native_packaged_dock_activation -- --ignored --exact --nocapture --test-threads=1
    if ($LASTEXITCODE -ne 0) { throw 'Native package activation acceptance failed.' }
    $activation = Get-Content -LiteralPath (Join-Path $out 'activation-report.json') -Raw | ConvertFrom-Json
    if (-not $activation.passed) { throw 'The native acceptance report did not pass.' }
    $report.passed = $true
} catch {
    $report.error = $_.Exception.Message
    throw
} finally {
    if ($registrationAttempted -and -not $registered) { $registered = Get-AppxPackage -Name 'Pleamar.NativeDockTest' }
    if ($registered) {
        # The package belongs only to this test. Do not stop another application.
        $exe = Join-Path $registered.InstallLocation 'fixture.exe'
        Get-CimInstance Win32_Process | Where-Object { $_.ExecutablePath -eq $exe } | ForEach-Object {
            Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue
        }
        Remove-AppxPackage -Package $registered.PackageFullName
    }
    if ($trusted) { Remove-Item -LiteralPath ("Cert:\LocalMachine\TrustedPeople\" + $trusted.Thumbprint) }
    if ($certificate) { Remove-Item -LiteralPath ("Cert:\CurrentUser\My\" + $certificate.Thumbprint) }
    $report.cleanup.package_removed = -not [bool](Get-AppxPackage -Name 'Pleamar.NativeDockTest')
    $report.cleanup.certificate_removed = $null -eq $trusted -or -not (Test-Path -LiteralPath ("Cert:\LocalMachine\TrustedPeople\" + $trusted.Thumbprint))
    $report | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $out 'package-report.json') -Encoding UTF8
}
