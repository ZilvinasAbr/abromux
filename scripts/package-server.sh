#!/usr/bin/env bash
# Package abromux-server on its own, for a machine without the app (a Linux
# VPS; docs/server-plan.md). Plain cargo: no Zig, no Wayland.
#
#   scripts/package-server.sh            # dist/abromux-server-<version>-<arch>-<os>.tar.gz
#   scripts/package-server.sh --vt       # with server-side parsing (libghostty-vt:
#                                        # scripts/build-libghostty.sh first)
#   scripts/package-server.sh --deb      # also dist/abromux-server_<version>_<arch>.deb
#                                        # (needs dpkg-deb, so on Debian/Ubuntu)
#
# The tarball unpacks into a prefix (tar -xzf .. -C ~/.local --strip-components=1):
#   bin/abromux-server, bin/abromux -> abromux-server (the commands agent hooks
#   and scripts call), lib/systemd/user/abromux-server.service,
#   share/doc/abromux-server/README.md, LICENSE.
# The .deb puts the same under /usr; `systemctl --user enable --now
# abromux-server` (and `loginctl enable-linger`) runs it at boot.
set -euo pipefail
cd "$(dirname "$0")/.."

features=()
deb=0
for a in "$@"; do
  case "$a" in
    --vt) features=(--features vt) ;;
    --deb) deb=1 ;;
    *) echo "package-server.sh: unknown option $a" >&2; exit 2 ;;
  esac
done

version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
cargo build --release -p abromux-server "${features[@]}"
bin=target/release/abromux-server
arch=$(uname -m)
os=$(uname -s | tr '[:upper:]' '[:lower:]')
name="abromux-server-$version-$arch-$os"
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
root="$stage/$name"
mkdir -p "$root/bin" "$root/lib/systemd/user" "$root/share/doc/abromux-server"
install -m755 "$bin" "$root/bin/abromux-server"
ln -s abromux-server "$root/bin/abromux"
cat > "$root/lib/systemd/user/abromux-server.service" <<'EOF'
[Unit]
Description=abromux server: keeps abromux terminals running
Documentation=https://github.com/ZilvinasAbr/abromux/blob/main/docs/server-plan.md

[Service]
# %h is the home folder; the socket is where `abromux-server --stdio` looks.
ExecStart=/usr/bin/env abromux-server serve --socket %h/.local/state/abromux/server.sock --no-idle-exit
Restart=on-failure
KillMode=control-group

[Install]
WantedBy=default.target
EOF
cp LICENSE "$root/"
cat > "$root/share/doc/abromux-server/README.md" <<EOF
# abromux-server $version

Keeps abromux terminals running on this machine, for the abromux app on
another one (docs/server-plan.md, docs/guide.md "Remote workspaces").

- The app reaches it with \`ssh HOST abromux-server --stdio\` and starts it
  when it isn't running; or run it as a service:
  \`systemctl --user enable --now abromux-server\` (and
  \`loginctl enable-linger \$USER\` to keep it up while logged out), or
  \`abromux-server install [--tailnet]\` to write the unit yourself.
- \`abromux-server hooks install\` sets up Claude Code, Codex and others'
  hooks here, so their state shows in the app.
- \`--tailnet\`: also on this machine's Tailscale addresses, with a web
  client at http://<host>:7297/ for a phone.
EOF
mkdir -p dist
tar -czf "dist/$name.tar.gz" -C "$stage" "$name"
(cd dist && shasum -a 256 "$name.tar.gz" > "$name.tar.gz.sha256")
echo "dist/$name.tar.gz"

if [[ $deb == 1 ]]; then
  command -v dpkg-deb >/dev/null || { echo "package-server.sh: --deb needs dpkg-deb" >&2; exit 1; }
  darch=$(dpkg --print-architecture)
  pkg="$stage/deb"
  mkdir -p "$pkg/DEBIAN" "$pkg/usr/bin" "$pkg/usr/lib/systemd/user" "$pkg/usr/share/doc/abromux-server"
  install -m755 "$bin" "$pkg/usr/bin/abromux-server"
  install -m644 "$root/lib/systemd/user/abromux-server.service" "$pkg/usr/lib/systemd/user/"
  install -m644 "$root/share/doc/abromux-server/README.md" LICENSE "$pkg/usr/share/doc/abromux-server/"
  cat > "$pkg/DEBIAN/control" <<EOF
Package: abromux-server
Version: $version
Architecture: $darch
Maintainer: Zilvinas Abromavicius
Section: admin
Priority: optional
Depends: libc6
Suggests: openssh-server, tailscale
Homepage: https://github.com/ZilvinasAbr/abromux
Description: abromux server: keeps abromux terminals running
 Runs terminals (and the coding agents in them) on this machine for the
 abromux app on another one, over ssh or Tailscale; they keep running
 whether or not the app is open. Includes agent hooks and a web client.
EOF
  # /usr/bin/abromux would clash with the app's package: the link is the
  # tarball's only. Agent hooks point at abromux-server itself.
  dpkg-deb --root-owner-group --build "$pkg" "dist/abromux-server_${version}_${darch}.deb" >/dev/null
  echo "dist/abromux-server_${version}_${darch}.deb"
fi
