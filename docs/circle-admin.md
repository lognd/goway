# Circle admin, renewal and policy

## Admin set

An admin must first be a circle member with the `lead` role under its admin key.

- `goway circle admin card [--name N]` prints a card for this machine's admin key
  (named `<machine>-admin`); an existing admin admits it with `goway circle admit`.
- `goway circle admin add NAME` / `remove NAME` appends an AddAdmin / RemoveAdmin op.
  With 1 or 2 admins one signature applies it at once.
- With 3 or more admins the op needs 2 distinct admin signers. Pass
  `--cosigner OTHER [--out FILE]` to write a half-signed proposal, then the other
  admin runs `goway circle admin cosign FILE` to finish and append it.
  Without a cosigner the command is refused.

Removing an admin bumps the roster epoch.

## Renewal

`goway circle renew [--force]` reissues the node certificate when it is missing or past
half of its 14 day TTL; `--force` reissues regardless.

## Policy

`goway circle policy open` accepts every circle member; `goway circle policy listed --lead NAME ...`
accepts only the named leads. `--file` overrides the `authorized_keys` target and
`--windows-admin` targets the Windows administrators file. `listed` needs at least one
known lead; `open` takes none.
