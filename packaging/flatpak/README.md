# Flatpak

CI builds this manifest with a `type: dir` source (`path: ../..`) so the
workspace checkout is what gets packaged.

Flathub will not accept that. A Flathub submission needs a `git` source with
a release tag and a pinned commit, for example:

```yaml
sources:
  - type: git
    url: https://github.com/ej3mpl0/openPDF
    tag: v0.1.0
    commit: <full-sha>
  - cargo-sources.json
```

Keep `cargo-sources.json` in sync with `Cargo.lock`
(`./generate-cargo-sources.sh`, checked by `./check-cargo-sources.py`).
