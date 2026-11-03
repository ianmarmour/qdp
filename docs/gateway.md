# Gateway Setup

QDP Gateway gives people outside your organization's network access to the
computers inside it, through one public address. People sign in with your
directory (Active Directory or LDAP) and an authenticator app, and the
gateway connects them to the computers they're allowed to reach. It never
sees what they stream.

## Install

On Fedora and RHEL:

```bash
sudo dnf install https://github.com/ianmarmour/qdp/releases/latest/download/qdp-gateway-linux-x86_64.rpm
```

On Debian and Ubuntu:

```bash
curl -LO https://github.com/ianmarmour/qdp/releases/latest/download/qdp-gateway-linux-x86_64.deb
sudo apt install ./qdp-gateway-linux-x86_64.deb
```

On Windows Server, install
[`qdp-gateway-windows-x64.msi`](https://github.com/ianmarmour/qdp/releases/latest/download/qdp-gateway-windows-x64.msi).

## Configure

1. Edit the configuration, `/etc/qdp-gateway/gateway.toml` on Linux or
   `C:\ProgramData\qdp-gateway\gateway.toml` on Windows. Its comments explain
   every setting; at least set your directory and a route to the computers.
2. Put the directory account's password in `directory-password`, beside the
   configuration.
3. Check the configuration, and a user's sign-in:

   ```bash
   sudo qdp-gateway check
   sudo qdp-gateway login alice
   ```

## Expose it to the internet

The gateway listens on UDP port 443. Forward that port to it from your
public address, and open it in any firewall in between; the Windows
installer opens the Windows firewall for you. Then start the service:

```bash
sudo qdp-gateway service install
```

## Grant access to computers

On each computer people should reach, trust the gateway with the key that
`qdp-gateway keys` prints. It's the same for every computer:

```bash
qdp serve --gateway-key KEY
```

`--gateway-group` admits only the gateway's users in one group. Which users
may open which computers is set by the configuration's routes and hosts.

## Connect

In the apps, set the gateway's address and your user name in settings: see
[Using the apps](setup/apps.md#through-your-organizations-gateway). From a
terminal:

```bash
qdp connect desk-12 --gateway gateway.example.com --user alice
```

`qdp-gateway help` and `qdp help` list every command and option.
