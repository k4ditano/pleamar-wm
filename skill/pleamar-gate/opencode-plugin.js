// pleamar-gate: opencode's permission requests go through the pleamar-wm session's gate
// (`pleamar-wm gate ask`): the person's rules, or the person from their shell, decide; with
// no decision, opencode asks on its own screen as ever. Written by `pleamar-wm gate setup`;
// `pleamar-wm gate setup --undo` takes it away.
export const PleamarGate = async ({ client, directory }) => {
  return {
    event: async ({ event }) => {
      if (event?.type !== "permission.asked" || !process.env.PLEAMAR_SOCKETS) return
      const p = event.properties || {}
      const what = (p.patterns || []).join(" ") || p.title || ""
      let out = ""
      try {
        const proc = Bun.spawn(
          ["pleamar-wm", "gate", "ask", "--agent", "opencode", "--tool", String(p.permission || p.type || "?"), "--cwd", directory || process.cwd(), "--", what],
          { stdout: "pipe", stderr: "ignore" },
        )
        out = (await new Response(proc.stdout).text()).trim()
      } catch {
        return
      }
      if (out !== "allow" && out !== "deny") return
      const reply = out === "allow" ? "once" : "reject"
      try {
        if (client?.permission?.reply) await client.permission.reply({ requestID: p.id, reply })
        else if (client?.postSessionIdPermissionsPermissionId)
          await client.postSessionIdPermissionsPermissionId({ path: { id: p.sessionID, permissionID: p.id }, body: { response: reply } })
      } catch {}
    },
  }
}
