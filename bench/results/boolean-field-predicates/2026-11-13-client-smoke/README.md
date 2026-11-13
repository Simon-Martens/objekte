# Native desktop predicate smoke

The native Objekte desktop ran on task-owned headless Wayland using `scripts/gui/run_desktop.sh prepare`. A five-file JSON fixture contained a missing `x`, `x: null`, `x: ""`, `x: 0`, and `x: "text"`.

The filter builder's standard predicate menu offered Nonempty, Is string, Is number, Is array, Is null, Is object, and Exists. Selecting Nonempty and True serialized `x:? = true` in the authoritative filter text and returned exactly `text.json` and `zero.json` ([screenshot](predicate-result-closed.png)). Selecting False serialized `x:? = false` and returned exactly `empty.json`, `missing.json`, and `null.json` ([screenshot](predicate-false-results.png)). [Builder screenshot](predicate-false.png) shows the False selection. The desktop process was stopped with `scripts/gui/run_desktop.sh cleanup`; task-owned artifacts remain in ignored `target/gui-validation`.
