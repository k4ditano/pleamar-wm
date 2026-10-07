# pleamar-wm, built next to pleamar's source (it is a path dependency,
# `../pleamar`), with its session for the login screen.
{
  lib,
  rustPlatform,
  runCommand,
  pkg-config,
  makeWrapper,
  pleamarSrc,
  wayland,
  libxkbcommon,
  libinput,
  seatd,
  systemdLibs,
  libgbm,
  libdrm,
  vulkan-loader,
  fontconfig,
  xwayland,
  swaybg,
  pam,
  pipewire,
  openssl,
  ffmpeg,
  grim,
  wf-recorder,
  wl-clipboard,
  libnotify,
}:
let
  version = (lib.importTOML ../Cargo.toml).package.version;
  # The two side by side, as the source expects them.
  src = runCommand "pleamar-wm-source" { } ''
    mkdir -p $out
    cp -r ${lib.cleanSourceWith { src = ../.; filter = path: _type: !(lib.hasInfix "/assets" path); }} $out/pleamar-wm
    cp -r ${pleamarSrc} $out/pleamar
    chmod -R u+w $out
  '';
in
rustPlatform.buildRustPackage {
  pname = "pleamar-wm";
  inherit version src;
  sourceRoot = "pleamar-wm-source/pleamar-wm";
  cargoLock.lockFile = ../Cargo.lock;

  nativeBuildInputs = [
    pkg-config
    makeWrapper
    # PipeWire's bindings are made from its headers (sharing the screen).
    rustPlatform.bindgenHook
  ];
  buildInputs = [
    wayland
    libxkbcommon
    libinput
    seatd
    systemdLibs
    libgbm
    libdrm
    pam
    pipewire
    # `pleamar-wm remote`'s direct way (WebRTC) encrypts with it.
    openssl
    # …and its video (`pleamar-wm-stream`, built with it): the card's encoder.
    ffmpeg
  ];

  # The session and the remote's video (`stream`).
  cargoBuildFlags = [ "--workspace" ];
  doCheck = false;

  postInstall = ''
    # The session: pleamar-session runs it as from a TTY; the login screen's
    # entry sets the desktop's name first and tells dbus and systemd where it is.
    install -Dm755 session.sh $out/bin/pleamar-session
    cat > $out/bin/pleamar-wm-session <<SESSION
    #!/bin/sh
    export XDG_CURRENT_DESKTOP=pleamar XDG_SESSION_DESKTOP=pleamar XDG_SESSION_TYPE=wayland PLEAMAR_WM_EXPORT=1
    exec $out/bin/pleamar-session "\$@"
    SESSION
    chmod 755 $out/bin/pleamar-wm-session
    install -Dm644 pleamar-wm.desktop $out/share/wayland-sessions/pleamar-wm.desktop
    substituteInPlace $out/share/wayland-sessions/pleamar-wm.desktop \
      --replace-fail "Exec=pleamar-wm-session" "Exec=$out/bin/pleamar-wm-session"
    install -Dm644 pleamar-portals.conf $out/share/xdg-desktop-portal/pleamar-portals.conf
    # Its own portal: sharing the screen, answered by pleamar-wm itself.
    install -Dm644 pleamar.portal $out/share/xdg-desktop-portal/portals/pleamar.portal
  '';

  postFixup = ''
    wrapProgram $out/bin/pleamar-wm \
      --prefix LD_LIBRARY_PATH : ${
        lib.makeLibraryPath [
          vulkan-loader
          wayland
          libxkbcommon
        ]
      } \
      --prefix PATH : ${
        lib.makeBinPath [
          xwayland
          swaybg
          # The agent's pictures and the remote desktop's: the monitors
          # taken (grim), as video (wf-recorder), the clipboard both ways,
          # and the notice when someone signs in from elsewhere.
          grim
          wf-recorder
          wl-clipboard
          libnotify
        ]
      } \
      --set-default FONTCONFIG_FILE ${fontconfig.out}/etc/fonts/fonts.conf
    wrapProgram $out/bin/pleamar-session --prefix PATH : $out/bin
  '';

  passthru.providedSessions = [ "pleamar-wm" ];

  meta = {
    description = "A Wayland compositor whose window manager is a pleamar scene";
    homepage = "https://github.com/k4ditano/pleamar-wm";
    license = lib.licenses.bsd3;
    mainProgram = "pleamar-wm";
    platforms = lib.platforms.linux;
  };
}
