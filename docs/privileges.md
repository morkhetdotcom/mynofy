# Operating system privileges at startup

On Linux, the CLI drops elevated Unix privileges before creating its
Tokio runtime. On macOS, the launcher must start it under an unprivileged
identity. Before establishing that identity, the CLI reads and validates
only the global UID and GID arguments. It opens configuration files, TLS
and client keys, adapter rules, and data and Admin sockets afterward.
This applies to every CLI command, including `admin`, `keygen`, and
`check-config`.

Run mynofy as a regular user or a dedicated service account.

```sh
mynofy serve --config /etc/mynofy/config.json
```

A Linux root launcher must supply both numeric target IDs.

```sh
mynofy --run-as-uid 1001 --run-as-gid 1001 serve --config /etc/mynofy/config.json
```

Replace the example IDs with those of the service account. The CLI rejects
UID or GID 0, the reserved value 4294967295, incomplete pairs and duplicate
arguments. It requires an explicit target for a root start. The options
can also follow the command name. A failed privilege drop terminates
startup.

## Startup sequence

For a Linux root start, the CLI clears supplementary groups and switches
the primary GID and UID permanently. On Linux it sets the real, effective
and saved IDs to the target, including when already unprivileged. This
happens in the application thread before workers and background tasks
start.

macOS rejects privileged starts even when target IDs are supplied. The
current `nix` wrapper does not expose the required Apple group APIs.
An unprivileged launcher can omit target IDs or pass its current UID and
GID. The CLI rejects mixed real and effective identities. Launch it from
a regular user account. Linux also supports the explicit root startup
path described above.

Linux clears effective, permitted, inheritable and ambient capabilities,
disables capability retention and sets `no_new_privs` permanently. With
`CAP_SETPCAP`, it also clears the capability bounding set. A regular
unprivileged user ordinarily cannot change its inherited bounding set.
The empty active capability sets and `no_new_privs` prevent an `exec`
of a setuid or setgid program or a program with file capabilities from
adding privilege. Programs started by the CLI adapter inherit these
restrictions.

An existing unprivileged identity may keep normal supplementary groups
if it cannot remove them. Linux rejects supplementary group 0 in that
case. On macOS the launcher manages those groups. An identity change
fails if inherited groups cannot be cleared safely. A dedicated service
account gives operators control over group membership. Capability and
`no_new_privs` behavior is Linux-specific. Non-Unix platforms have no
UID and GID options or operating system privilege drop.

## Files, networking and administration

The target user must have access to the configuration and keys, the
socket parent directory and the requested network port. The service
opens files and binds ports as that identity. It retains no capability
for privileged ports and never changes file owners automatically.
An existing socket path causes startup to fail. New Unix sockets belong
to the target UID and GID. The separate Admin socket checks the peer UID.
An Admin CLI launched from a Linux root shell needs the same explicit
target arguments.

With systemd, set `User=` and `Group=`. In a container, configure the
container UID and GID. The startup check also handles accidental
privileged invocations. Embedded server and client libraries leave the
host process identity to its launcher.

## Verification

`cargo test --locked --workspace` runs argument and Linux process tests.
On Linux the tests inspect every runtime thread through `/proc` for
real, effective, saved and filesystem IDs, capability sets and
`no_new_privs`. They also verify socket ownership and Admin access.
Root tests check that an invalid root invocation exits before file I/O
and that a valid invocation drops to a different UID and GID. The
adapter test checks that the child inherits the restrictions and cannot
return to UID or GID 0.

When tests run as root, CLI integration scenarios re-execute under
UID and GID 65534 using Python. The test runner keeps its identity.
This requires permission to set the child UID and GID and make the
build paths readable and executable by the child. Native macOS runtime
behavior is verified on macOS.
