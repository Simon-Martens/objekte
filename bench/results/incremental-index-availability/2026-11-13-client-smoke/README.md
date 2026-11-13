# Plan 1 interactive client smoke

These artifacts document manual and PTY interaction on the incremental-index-availability task branches, before the final performance repair. They are functional evidence, not construction benchmarks.

## Desktop

The native GPUI application ran in a headless Wayland session at 1920 × 1080. The tester used `bash scripts/gui/run_desktop.sh prepare`, `target/gui-validation/venv/bin/python scripts/gui/desktop_control.py status|click|type|screenshot`, and `bash scripts/gui/run_desktop.sh cleanup`. The four PNGs show a provisional 4,096/148,486-file result view, the complete physical-file catalogue during indexing, an edited query with provisional results at 102,400/148,486 files, and a final exact 275-match result with record preview. The GUI typing helper dropped the first character; the observed query was `useum`. No snapshot was written. The desktop subtask branch was at `8dec99240a`.

## TUI

The two JSON traces come from a real PTY session on a bounded 6,200-file, roughly 50 MiB fixture. The child was pinned to one CPU to make intermediate states visible. The normal trace records catalogue display before a searchable batch, typing `needle`, provisional matches after 2,048 files, replacing the query with `hay`, a final exact 6,032-match state, resize, and Ctrl+Q exit 0. The cancellation trace records Ctrl+C at catalogue availability and exit 130. This was a state-transition check, not a latency benchmark. The CLI/TUI subtask branch was at `d770085bec`.
