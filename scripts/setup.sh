#!/bin/bash -p
# The owner's one setup command, run from the checkout: scripts/setup.sh, or with one of
# keychain, maintenance or recovery. It starts the wizard with a cleared environment and
# /bin/bash -p, which the wizard checks before any stage. See docs/macos-confirmation.md.
case "$0" in */*) here="${0%/*}" ;; *) here=. ;; esac
exec /usr/bin/env -i HOME="$HOME" TERM="${TERM:-}" PATH=/usr/bin:/bin:/usr/sbin:/sbin \
  /bin/bash -p "$here/setup-macos-protection.sh" "$@"
