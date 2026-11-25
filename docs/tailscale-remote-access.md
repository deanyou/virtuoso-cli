# Remote Access over Tailscale

How to reach a Virtuoso compute host through Tailscale instead of a
corporate VPN or an SSH jump host. No code changes: this is a
deployment recipe.

## Why it works

Remote mode is SSH end to end. `vcli tunnel start` opens an SSH
connection to the compute host and tunnels to the daemon on top of
it, for both SSH backends (`openssh` and the pure-Rust `native`
russh transport). Anything that gives the client TCP reachability
to the host's sshd works as the carrier, and Tailscale is one such
carrier. Point `VB_REMOTE_HOST` at the host's tailnet address and
vcli cannot tell the difference.

## Standard setup

On the Virtuoso host:

```bash
tailscale up
tailscale ip -4        # note the 100.x address, or use the MagicDNS name
```

A stock sshd listens on all interfaces, which already covers the
Tailscale interface (`tailscale0`). No sshd change is needed.

On the client:

```bash
tailscale up           # same tailnet as the host
export VB_REMOTE_HOST=<host 100.x address or MagicDNS name>
export VB_SSH_KEY=~/.ssh/id_ed25519_vcli   # only if not your default key
vcli tunnel start
vcli skill exec 'getCurrentTime()'
```

Leave `VB_JUMP_HOST` unset. Reaching the host directly through the
tailnet is the point; the bastion drops out of the path.

## Verify

```bash
tailscale ping $VB_REMOTE_HOST   # "direct" is best; DERP relay also works
ssh $VB_REMOTE_HOST true
vcli session list
```

## When the host cannot run Tailscale

Managed CAD servers often forbid installing it. Two workarounds,
host untouched in the first case:

- **Subnet router.** Any tailnet machine on the host's LAN
  advertises that subnet into the tailnet. Set `VB_REMOTE_HOST` to
  the host's LAN IP; traffic routes through the subnet router.
- **Userspace mode.** Run `tailscaled` with
  `--tun=userspace-networking` on the host (no root, no TUN
  device). Inbound connections are handled by tailscaled and
  forwarded to the local sshd.

## Access control

Restrict the host's SSH port in the tailnet ACL to the devices and
identities that are supposed to drive Virtuoso, for example:

```json
{
  "acls": [
    {
      "action": "accept",
      "src": ["group:eda-clients"],
      "dst": ["tag:virtuoso-host:22"]
    }
  ]
}
```

This is a network-layer gate. It complements, and does not replace,
vcli's own operation layer (`VCLI_CAPABILITY`).

## Notes

- GUI automation (xdotool, screenshots) executes on the Virtuoso
  host itself, so X11 latency never crosses the tailnet. Only SSH
  commands and result payloads do. A DERP-relayed connection is
  therefore fine for GUI work; it only slows bulk result transfer.
- Tailscale SSH (`tailscale up --ssh`) can replace the host sshd
  for the `openssh` backend. Prefer the host's regular sshd with
  key auth when using the `native` backend, which expects standard
  SSH key authentication.
