# Publishing VoiceGarden-SPD

## GitHub Releases (primary, automated)

Tags drive everything (`.github/workflows/release.yml`):

```bash
$EDITOR Cargo.toml              # bump [workspace.package] version
git commit -am "chore: release vX.Y.Z"
git tag vX.Y.Z && git push --tags
```

Per release, for **x86_64 and aarch64**:
- tarball (module + refresh + CLI + config + install.sh) with a stable `latest-<arch>` alias
- `.deb` (postinst registers in `/etc/speech-dispatcher/speechd.conf`, warns on speechd < 0.12; prerm unregisters)
- `.rpm` (same scripts)
- install.sh (one-liner; prefers the native package, falls back to user-local)

A package smoke job installs the freshly built `.deb` on a clean runner
and speaks a real Piper model through a real speech-dispatcher before
the release publishes.

## AUR (manual, after each release)

`packaging/aur/voicegarden-spd-bin/PKGBUILD` — release tarball.
`packaging/aur/voicegarden-spd-git/PKGBUILD` — builds from git main.

```bash
# one-time setup: clone the AUR over ssh
git clone ssh://aur@aur.archlinux.org/voicegarden-spd-bin.git
cd voicegarden-spd-bin
cp /path/to/VoiceGarden-SPD/packaging/aur/voicegarden-spd-bin/PKGBUILD .
updpkgsums          # fill in the real sha256 per arch
makepkg --printsrcinfo > .SRCINFO
git add PKGBUILD .SRCINFO && git commit -m "vX.Y.Z" && git push
```

## Not packaged here (and why)

- **Flathub** — the module must live outside any sandbox where the
  speech-dispatcher daemon can exec it; only the planned GTK config app
  is Flatpak-able.
- **crates.io** — the deliverable is binaries + distro packages, not a
  library; the git dependency on rust-tts-wrapper is fine for that.
- **Debian/Fedora official repos** — worth doing once the project has a
  track record; the .deb/.rpm here are structured to be diff-friendly
  toward that (`debian/`-style scripts, no vendored deps beyond the
  BSD protocol units).
