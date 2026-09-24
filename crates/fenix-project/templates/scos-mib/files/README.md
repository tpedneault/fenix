# {{name}}

An SCOS-2000 MIB database (Database Import ICD {{icd}}).

- `mib/` holds the tables, one tab-separated `.dat` file each.
- `procedures/` holds Tcl telecommand procedures.

In Fenix, `SPC m t` looks up telecommands, `SPC m k` TM packets and
`SPC m p` TM parameters; `SPC m r` rereads the tables after editing.
