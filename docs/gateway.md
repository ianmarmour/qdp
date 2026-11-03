# The QDP gateway

`qdp-gateway` lets people outside an organization's network reach the QDP
hosts inside it through one public endpoint. Most people don't need it: it's
for organizations with a directory of their own, LDAP or Active Directory.
[Section 13](spec/13-gateway.md) of the specification defines the protocol.

- [How it works](#how-it-works)
- [Installing](#installing)
- [Setting it up](#setting-it-up)
- [Connecting through it](#connecting-through-it)
- [Configuration](#configuration)
- [Commands](#commands)

## How it works

- **Signing in.** Each user signs in with their domain credentials, which the
  gateway checks with the organization's directory over TLS, and then always
  with a TOTP code from an authenticator app such as Google Authenticator,
  Microsoft Authenticator or Authy. A user without one yet is shown a QR code
  to add it to their app the first time, and asked for a code at every
  sign-in after.
- **Tunnelling.** The gateway tunnels each user's QDP session to a host,
  end to end: it never reads the session. Users reach the hosts the gateway
  lists for them, or that its routes let them open by their names inside the
  network.
- **Vouching for users.** When a user opens a host, the gateway learns the
  host's key with a handshake of its own inside the network, vouches for the
  user to that key, and tells the client to verify the host by it. This
  trusts the network's names for hosts' identities, as users inside it do. A
  host listed in the configuration with its key pin is verified by that pin
  instead.

The gateway stands apart from QDP's apps and hosts, and runs as a service on
Linux and Windows servers.

## Installing

### Linux

On Fedora 40 or later and RHEL 9 or later, install the RPM:

```bash
sudo dnf install https://github.com/ianmarmour/qdp/releases/latest/download/qdp-gateway-linux-x86_64.rpm
```

On Debian 12 or later and Ubuntu 22.04 or later, install the Debian package:

```bash
curl -LO https://github.com/ianmarmour/qdp/releases/latest/download/qdp-gateway-linux-x86_64.deb
sudo apt install ./qdp-gateway-linux-x86_64.deb
```

Both create the user `qdp-gateway`, a hardened systemd unit, and an example
configuration in `/etc/qdp-gateway/gateway.toml` to edit.

Elsewhere, extract `qdp-gateway-linux-x86_64.tar.gz` and run
`sudo qdp-gateway service install`, which makes the same: the user, the unit
and, the first time, the example configuration.

### Windows

On Windows Server 2019 or later, install
[`qdp-gateway-windows-x64.msi`](https://github.com/ianmarmour/qdp/releases/latest/download/qdp-gateway-windows-x64.msi).
It installs:

- the gateway, on the `PATH`;
- its service, which runs as LocalService from boot;
- its folder, `C:\ProgramData\qdp-gateway`, readable only by the service and
  administrators, which holds its configuration, keys and log, with an
  example configuration to edit;
- a firewall rule for UDP port 443.

Upgrades keep the configuration and start the service again.

Without the installer, unzip `qdp-gateway-windows-x86_64.zip` and run
`qdp-gateway service install` from an administrator's terminal. It makes the
same service and, once configured, opens the firewall for its port; run it
again after moving the gateway off port 443.

## Setting it up

Deploying the gateway takes one setting on each side, none of them per host:

1. **On the gateway:** the directory, and a route to the hosts' network, such
   as every name under `desktops.corp.example.com` in `10.20.0.0/16` for the
   group `QDP Users`.
2. **On every host:** the gateway's assertion key, which `qdp-gateway keys`
   prints and which is the same for every host, as
   `qdp serve --gateway-key KEY`, or in the host's service options.
   Configuration management can set it across a fleet.

Step by step, on Linux:

1. Edit `/etc/qdp-gateway/gateway.toml` (see [Configuration](#configuration)).
2. Put the directory account's password in `directory-password` beside it,
   readable only by the gateway.
3. Check the configuration and the directory account:

   ```bash
   sudo qdp-gateway check
   ```

4. Check a user's sign-in, and whether they may open a host:

   ```bash
   sudo qdp-gateway login alice build-01
   ```

5. Start the service, now and at every boot:

   ```bash
   sudo qdp-gateway service install
   ```

6. Print the pin clients pin the gateway by, and the key hosts trust:

   ```bash
   sudo qdp-gateway keys
   ```

7. On each host, trust the gateway, optionally admitting only one group:

   ```bash
   qdp serve --gateway-key KEY --gateway-group "QDP Users"
   ```

On Windows, edit `C:\ProgramData\qdp-gateway\gateway.toml`, put the password
in `directory-password` beside it, and start the service.

## Connecting through it

In the apps, set the gateway in their settings and add computers by their ID
on the gateway or their name inside the network: see
[Connecting](connecting.md#through-your-organizations-gateway).

With the `qdp` command, name the host and the gateway:

```bash
qdp connect build-01 --gateway gw.corp.example.com --user alice
```

The first time, it asks for alice's domain password and shows a QR code to
scan with her authenticator app, then asks for a code from the app; a
mistyped code is asked for again with the same QR code. The gateway then
vouches for her to the host, and the client verifies the host's key with the
pin the gateway knows, refusing a host whose key differs from one it pinned
itself. Later connections sign in with the session token kept in the client's
identity directory, without asking, until it expires.

`--gateway-pin PIN` pins the gateway's key, from `qdp-gateway keys`, where its
certificate isn't from a certificate authority the system trusts.

## Configuration

The gateway reads `/etc/qdp-gateway/gateway.toml` on Linux and
`C:\ProgramData\qdp-gateway\gateway.toml` on Windows, unless `--config` names
another file. The installers' [example](../packaging/gateway.toml) explains
each setting in place.

### General

| Setting | What it is | Default |
|---|---|---|
| `name` | The name users see when signing in, at most 64 bytes | required |
| `listen` | The UDP address to listen on; port 443 passes networks that allow HTTP/3 | `0.0.0.0:443` |
| `state_directory` | Where the gateway keeps its keys and TOTP secrets | `/var/lib/qdp-gateway`, or its ProgramData folder on Windows |

### `[tls]`

Optional: a certificate from the organization's certificate authority, in
place of the self-signed one the gateway makes itself. With it, clients need
no `--gateway-pin`.

| Setting | What it is |
|---|---|
| `certificate` | The certificate chain in PEM, end-entity certificate first |
| `private_key` | The private key in PEM |

### `[directory]`

| Setting | What it is | Default |
|---|---|---|
| `url` | The directory: `ldaps://`, or `ldap://` with `starttls` | required |
| `starttls` | Upgrade an `ldap://` connection with StartTLS | `false` |
| `ca_certificate` | The certificate authority the directory's certificate chains to, in PEM, if the system doesn't trust it | |
| `bind_dn` | The account the gateway finds users with | required |
| `bind_password_file` | The file holding that account's password, which only the gateway may read | required |
| `base_dn` | Where users are searched for | required |
| `user_filter` | The filter that finds a user, with `{user}` for the name they type | `(&(objectClass=user)(sAMAccountName={user}))` |
| `name_attribute` | The attribute of the user's name | `sAMAccountName` |
| `display_name_attribute` | The attribute of the user's full name | `displayName` |
| `group_attribute` | The attribute listing the user's groups | `memberOf` |
| `timeout_seconds` | How long to wait for the directory | `10` |

For OpenLDAP rather than Active Directory, set `user_filter` to
`(&(objectClass=inetOrgPerson)(uid={user}))` and `name_attribute` to `uid`.

### `[totp]`

TOTP is always required.

| Setting | What it is | Default |
|---|---|---|
| `enrollment` | `"first-login"`: a user without a secret enrolls one as they first sign in. `"admin-only"`: only `qdp-gateway totp enroll` enrolls, for organizations that won't let a password alone choose the second factor | required |
| `issuer` | The issuer authenticator apps show beside the codes | the gateway's `name` |

### `[sessions]`

| Setting | What it is | Default |
|---|---|---|
| `token_lifetime_hours` | How long a sign-in lasts, 1 to 168 hours | `12` |

### `[assertions]`

| Setting | What it is | Default |
|---|---|---|
| `enabled` | Whether the gateway vouches for users to hosts | `true` |
| `lifetime_seconds` | How long an assertion lasts, 1 to 300 seconds | `120` |
| `groups` | The groups hosts are told users are in, of those they're in | |
| `learn_host_keys` | Learn the keys of hosts not listed with a pin; `false` vouches only for listed hosts | `true` |

### `[limits]`

Limits on failed sign-ins, which keep attempts through the gateway below the
directory's own lockout threshold.

| Setting | What it is | Default |
|---|---|---|
| `user_failures` | Failures for one user name within the window, after which the gateway stops trying it | `5` |
| `address_failures` | Failures from one address within the window, after which the gateway refuses it | `20` |
| `window_minutes` | The window | `15` |

### `[[route]]`

Hosts users may open by their names or addresses inside the network. Hosts
reached by a route need only trust the gateway's key.

| Setting | What it is | Default |
|---|---|---|
| `names` | The names allowed: exact, or `*.` and a domain for every name in it; empty allows any name that resolves into `networks` | |
| `networks` | The networks every address of a host must lie in, as `10.20.0.0/16`; empty allows any address | |
| `ports` | The ports allowed | `[4433]` |
| `groups` | The groups whose members may use the route | |
| `users` | Users who may use the route besides those groups' members | |

### `[[host]]`

Hosts listed to users by ID.

| Setting | What it is | Default |
|---|---|---|
| `id` | The host's ID on the gateway, at most 255 bytes | required |
| `name` | Its name for users, at most 64 bytes | required |
| `description` | More about it, at most 255 bytes | |
| `address` | Where it listens, as a name or address with a port | required |
| `pin` | Its key pin, from `qdp pin`; without it the gateway learns the key, if `learn_host_keys` allows | |
| `server_name` | The name its certificate is for | `qdp-host` |
| `groups` | The groups whose members may reach it | |
| `users` | Users who may reach it besides those groups' members | |

## Commands

Each command takes `--config FILE`.

| Command | What it does |
|---|---|
| `qdp-gateway serve` | Serve users; the service runs this |
| `qdp-gateway service install` | Run the gateway as a service from boot: a systemd unit as the user `qdp-gateway` on Linux, a Windows service as LocalService on Windows. Without a configuration, writes an example to edit first |
| `qdp-gateway service uninstall` | Stop and remove the service; its configuration and keys stay |
| `qdp-gateway service status` | Show whether the service runs |
| `qdp-gateway keys` | Print the pin clients pin the gateway by, and the assertion key hosts trust |
| `qdp-gateway check` | Check the configuration, and that the directory account can bind |
| `qdp-gateway login USER [TARGET]` | Check USER's password, from `QDP_PASSWORD`, and show who they are, which hosts they may reach and their groups; with TARGET, whether they may open it |
| `qdp-gateway totp enroll USER` | Make USER a new TOTP secret and print the `otpauth://` URI that adds it to an authenticator app (`qrencode -t ansiutf8 URI` shows it as a QR code) |
| `qdp-gateway totp remove USER` | Forget USER's secret, so that they enroll again |
| `qdp-gateway totp list` | List the users with a secret |
