### Added
- The interactive REPL gained a handful of shell conveniences:
  - **`:!<cmd>`** (alias **`:shell <cmd>`**) runs a command in the system shell with
    the REPL's stdio inherited.
  - **`:cd [dir]`** changes the working directory (home by default; a leading `~` is
    expanded), so relative `:load` paths and file effects resolve there.
  - **`:edit [file]`** opens a file in `$EDITOR` (`$VISUAL`, else a platform default)
    — the last `:load`'d file by default — and reloads it afterwards if it was the
    loaded one.
  - **`:script <file>`** runs each line of a file as REPL input (declarations,
    expressions, and `:`-commands), reusing the normal multi-line accumulation; a
    `:quit` inside the script exits the REPL.
