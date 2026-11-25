# Terminal fixtures

Raw output streams recorded from the real programs on a PTY of **69 × 28** cells, the grid the
side pane holds at the screenshot size (960 × 587 points at 150 %, chat ratio 0.32). Tests
replay them (`src/tests.rs`), and both front ends replay them for the screenshots, so the egui
and WPF pictures compare cell for cell.

Recorded with `examples/record.rs` (Linux; bash, vim, less, pwsh and htop on `PATH` or given by
`HTOP`/`HTOP_LIBS`), which answers the programs' queries — cursor position, device attributes,
colours — with `yaac-term` itself:

```text
cargo run -p yaac-term --example record -- 69 28 crates/yaac-term/fixtures [names…]
```

The recorder builds a small project in `/tmp/yaac-fixture` (`~/work/auth`, a git repository with
one commit and two changes), sets `PS1` to `dev@devbox:\w$`, and replaces the recording user's
name with `dev`.

| Fixture | What | Screenshots |
| --- | --- | --- |
| `bash-prompt.vt` | bash readline: `ls`, `git status -sb`, `git log`, `wc -l`, `grep -c`, then `cargo build --rel` typed, not run | `bash-prompt`; `selection` selects rows 8:0 – 14:8 (inclusive, viewport cells); the panel scenes (`panel-open`, `panel-maximised`, `panel-fullchat`) at the panel's own size |
| `search.vt` | `tail -n 24 docs/sessions.log` | `search`: the query `denied`, the newest match current |
| `exited.vt` | `ls`, `grep -n denied …`, then `exit 3` | `exited`: "Process exited with code 3" |
| `vim.vt` / `vim-light.vt` | `vim -u NONE` with syntax, numbers, ruler, the cursor at 20,9; `background=dark` / `light` (what a real vim picks after asking the terminal, OSC 11) | `vim` (dark / light), `vim-full` |
| `htop.vt` | htop 3.3 with CPU/Memory/Swap and Tasks/Load/Uptime meters, six named processes, one busy | `htop`, `htop-full` |
| `powershell-prompt.vt` | PowerShell 7 with PSReadLine: `Get-ChildItem -Name src`, `$PSVersionTable.PSVersion.Major`, then a `Get-Content … \| Select-String 'cargo'` line typed, not run | `powershell-prompt` |
| `vim-session.vt` | vim opened, text inserted, `:q!` | — (tests: back to the shell screen) |
| `less.vt` | `less -R`, a page, `/denied`, `q` | — (tests) |

Screenshots: `yaac --demo --screenshots <dir> --only term-bash-prompt,term-vim,…` writes
`<theme>-<scene>.png` at 1440 × 880.
