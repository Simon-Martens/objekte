# mmap-index-v1 GUI storage validation

Date: 2026-11-15  
Branch: `integration/mmap-index-v1`  
Fixture: `target/gui-validation/mmap-fixture` with `a.json`, `b.json`, and `c.json`  
Session: task-owned headless Sway 1.11, 1920 × 1080, scale 1, 60 Hz

## Automated control script

The repository's headless environment harness was installed user-locally and run:

```bash
bash scripts/gui/install_user_dependencies.sh
bash scripts/gui/run_gui_validation.sh prepare
bash scripts/gui/run_gui_validation.sh exercise 400 132 34 173 76 214 500 450 180 214
```

Result:

```text
Fixture PASS: .../target/gui-validation/fixture.json
```

The saved fixture was:

```json
{"text":"Objekte ÄÖÜ äöü ß 123","checked":true,"action_count":1}
```

The screenshots `automated-after-type.png` and `automated-final.png` show the
Unicode text, checked flag, incremented action count, scrolled list, and saved
fixture path.

## Desktop storage controls

The production desktop binary was launched with:

```bash
bash scripts/gui/run_desktop.sh prepare
```

The Open dialog was exercised through the scripted `desktop_control.py`
commands. Storage presets were created with the CLI before GUI loading so the
fixture directory only contains source JSON:

```bash
./target/debug/obj --path target/gui-validation/mmap-fixture --save-index repo
./target/debug/obj --path target/gui-validation/mmap-fixture --save-index cache
./target/debug/obj --path target/gui-validation/mmap-fixture \
    --save-index target/gui-validation/mmap-custom-outside
```

Observed results:

- Build mode opened the fixture as a plain folder: [open-build.png](mmap-index-storage-gui/open-build.png).
- Load mode showed System cache, Repository, and Custom folder, plus
  check-after-load: [open-load-custom.png](mmap-index-storage-gui/open-load-custom.png).
- Repository load reported `Origin: Loaded from disk`, location
  `.../mmap-fixture/.objekte/.index`, 5.9 KiB mapped, 1 run:
  [loaded-repository.png](mmap-index-storage-gui/loaded-repository.png).
- System-cache load reported the cache namespace
  `.../.cache/objekte/indexes/095303c8...`, 5.9 KiB mapped, 1 run:
  [loaded-cache.png](mmap-index-storage-gui/loaded-cache.png).
- Custom load reported `Origin: Loaded from disk` for
  `target/gui-validation/mmap-custom-outside`, 5.9 KiB mapped, 1 run:
  [loaded-custom.png](mmap-index-storage-gui/loaded-custom.png).
- Check-after-load reported `Last check: Checked this session · complete`,
  `Source files: Source files current`, and `Source state: Checked`:
  [checked-after-load.png](mmap-index-storage-gui/checked-after-load.png).
- Changing a source file and selecting **Check for changes** advanced the
  revision and showed `Changes: Unsaved`; saving wrote generation
  `f98549cd-3fe5-4378-91c0-1126f80de4d2` with 2 objects:
  [unsaved-after-check.png](mmap-index-storage-gui/unsaved-after-check.png)
  and [saved-custom.png](mmap-index-storage-gui/saved-custom.png).
- An invalid custom folder produced an inline storage error while the current
  results, `Index ready`, and `Files checked` state remained visible:
  [load-error-preserves-session.png](mmap-index-storage-gui/load-error-preserves-session.png).

The headless session and task-owned application were stopped after validation;
screenshots were retained under `target/gui-validation/` and the selected
evidence above was copied into this directory.
