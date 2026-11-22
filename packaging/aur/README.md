# Linux packages

## AUR: `abromux-bin`

`abromux-bin/PKGBUILD` installs a release's Linux tarball
(`scripts/package-linux.sh --dist`) under `/usr`: the `abromux` binary, the
desktop entry `dev.abromux.app.desktop` and the icons. This is how Omarchy
users install things (`yay -S abromux-bin`).

After a release whose tarballs are uploaded to GitHub:

```bash
cd packaging/aur/abromux-bin
sed -i "s/^pkgver=.*/pkgver=<version>/; s/^pkgrel=.*/pkgrel=1/" PKGBUILD
updpkgsums                      # fills sha256sums from the release (pacman-contrib)
makepkg --printsrcinfo > .SRCINFO
makepkg -si                     # try it
```

Then commit `PKGBUILD` and `.SRCINFO` to the AUR's git repository
(`ssh://aur@aur.archlinux.org/abromux-bin.git`).

The installed app doesn't update itself (pacman owns its files); it says
when a new version is out and to update with the AUR helper.

## AUR: `abromux-server`

`abromux-server/PKGBUILD` builds the server alone from a release's source
(plain cargo), for a machine without the app: `/usr/bin/abromux-server` and
a systemd user unit (`systemctl --user enable --now abromux-server`). The
same steps as above after a release. `scripts/package-server.sh` makes its
tarball and a `.deb` (`--deb`, on Debian/Ubuntu).

## AppImage and tarball

Both come from `scripts/package-linux.sh --dist` and go on the GitHub
release. The AppImage updates itself; the tarball unpacks into a prefix
(`tar -xzf abromux-<version>-x86_64-linux.tar.gz -C ~/.local --strip-components=1`)
and, like the AUR package, only says when there's an update.

No Flatpak: its sandbox would hide the shells and agents the app runs.
