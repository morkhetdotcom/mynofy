# YAML configuration

YAML is the preferred format for service configuration. The loader
recognizes `.yaml` and `.yml` case-insensitively. Other or missing
extensions select strict JSON. A parse error stops loading in the selected
format. Both formats use the same `Config` types, defaults and validation.

```yaml
# See config/scenarios/unix.yaml for the full example.
service:
  id: local
  unix_socket: run/mynofy.sock
  admin_socket: run/mynofy-admin.sock
security_storage:
  directory: security
  queue_capacity: 128
  compact_after_records: 4096
```

Relative file, socket and security paths resolve against the configuration
directory. `Config::parse` validates input without resolving paths or
opening stores. `Config::load` reads the file and resolves its paths.
The setup scripts generate `config.yaml` with a separate `security`
directory for each demo and preserve existing `.local` files.

## Supported YAML profile

The Rust parser supplies the YAML grammar. mynofy maps its supported
events into the existing JSON data model. The project's minimum Rust
version is 1.88. The parser's package declares Rust 1.85. [Saphyr](https://github.com/saphyr-rs/saphyr)

Supported syntax includes comments, block and flow mappings and sequences,
single and double quotes, and literal and folded multiline strings.
Mapping keys must be scalar. Duplicate decoded keys are rejected at any
depth. Unknown fields and incorrect types are rejected as with JSON.

The loader rejects anchors, aliases, tags including `!!str`, merge keys
`<<`, complex mapping keys and multiple documents. Includes and
environment-variable expansion are unavailable. Configurations are limited
to 32 MiB of UTF-8 input. YAML also has a maximum depth of 64 and a limit
of one million parser events. The exported YAML limit is 32 MiB.

Unquoted scalars follow these rules.

- `true` and `false`, including their initial-capital and uppercase
  variants, are booleans.
- `null`, `Null`, `NULL`, `~` and empty values represent null.
- Numbers use JSON syntax, such as `12`, `-3` and `1.5`. The field type
  determines which numbers are valid. Limits require nonnegative integers.
- Other scalars are strings. Date and time fields validate those strings
  as RFC 3339. `yes` and `no` are strings. Hexadecimal, octal and numbers
  with leading zeros are unavailable as alternative forms for numeric limits.

Quote strings such as `"true"`, `"null"` and `"0012"` when necessary.
Exports quote strings and use two-space indentation. Conversion and export
regenerate the formatting and discard comments.

## Startup and existing configurations

```sh
cargo run --locked -- check-config --config .local/unix/config.yaml
cargo run --locked -- serve --config .local/unix/config.yaml
```

The usual Unix privilege rules apply. A Linux root start requires explicit
target IDs. JSON remains supported, and tests confirm that the four JSON
scenario templates are equivalent to their YAML counterparts.

Convert an existing installation with:

```sh
mynofy config-convert --config config.json --out config.yaml
```

The command validates the source and writes a new private file. The target
extension selects YAML or JSON. Paths become absolute, including the
effective security directory. Conversion leaves the journal untouched.

An older configuration with an implicit security path derives it from the
full configuration filename. Renaming `config.json` to `config.yaml`
would therefore change the derived path from `config.json.security/`
to `config.yaml.security/`. Conversion preserves the original association
by adding an explicit path. For a manual migration, set
`security_storage.directory` to the existing store and verify it before
running `serve`.

## Admin export and internal formats

```sh
mynofy admin export --socket run/mynofy-admin.sock --out snapshot.yaml
```

Admin export uses YAML for `.yaml` and `.yml` output and JSON otherwise.
Without `--out`, it writes JSON to stdout. A file export creates a new file
and includes the current security path.

YAML applies to service configuration only. The Admin and client JSONL
interfaces, adapter rules, catalogs and security journal retain their
existing formats.
