#!/usr/bin/env bash
# Transactional generation + user-unit activation. Called only after consent.
# Holds the same flock as bootstrap until service health or complete rollback.
set -euo pipefail
log() { printf '[update] %s\n' "$*"; }
die() { printf '[update] error: %s\n' "$*" >&2; exit 1; }

PREFIX="${DSH_OCEAN_DATA_DIR:?runtime prefix required}"
PREFIX="$(cd -- "$PREFIX" && pwd -P)"
[ "$PREFIX" != / ] && [ ! -L "$PREFIX/generations" ] || die 'invalid runtime prefix'
UNIT="${DSH_OCEAN_UNIT_PATH:?unit path required}"
SERVICE_UNIT="${DSH_OCEAN_SERVICE_UNIT:-dsh-web.service}"
[[ "$SERVICE_UNIT" = dsh-web.service || "$SERVICE_UNIT" =~ ^dsh-ocean-test-[A-Za-z0-9-]+\.service$ ]] || die 'unexpected unit namespace'
[ "${#SERVICE_UNIT}" -le 128 ] && [ "${UNIT##*/}" = "$SERVICE_UNIT" ] || die 'unexpected unit name'
export DSH_OCEAN_SERVICE_UNIT="$SERVICE_UNIT"
UNIT_DIR="$(cd -- "$(dirname -- "$UNIT")" && pwd -P)"
UNIT="$UNIT_DIR/$SERVICE_UNIT"
[ -f "$UNIT" ] && [ ! -L "$UNIT" ] || die 'only a regular managed unit can be updated'
exec 9>"$PREFIX/.bootstrap.lock"
flock -n 9 || die 'another runtime operation is already in progress'
export DSH_OCEAN_INHERITED_LOCK=9
export DSH_OCEAN_DATA_DIR="$PREFIX"

root_for() {
  [[ "$1" =~ ^generations/gen\.[[:alnum:]]+$ ]] || die 'invalid generation selector'
  [ -d "$PREFIX/$1" ] && [ ! -L "$PREFIX/$1" ] || die 'generation is missing'
  printf '%s\n' "$PREFIX/$1"
}
[ -L "$PREFIX/current" ] || die 'a generation must already be selected'
OLD_TARGET="$(readlink "$PREFIX/current")"
OLD_ROOT="$(root_for "$OLD_TARGET")"
[ "$OLD_ROOT" = "${DSH_OCEAN_EXPECTED_ROOT:?expected generation required}" ] || die 'runtime selection changed; retry update'
# Compare every directive, normalizing only one precisely generated PATH line.
# No extra assignments, unescaped quotes, continuation or custom directives hide here.
# Pure bash: the service may have a restricted PATH.
unit_fingerprint() {
  local line paths=0 path_pattern='^Environment="PATH=([^"\\%]|%%|\\[\\"nrt])*"$'
  while IFS= read -r line || [ -n "$line" ]; do
    if [[ "$line" =~ $path_pattern ]]; then
      paths=$((paths + 1))
      [ "$paths" -eq 1 ] || return 1
      printf '%s\n' 'Environment="PATH=<generated>"'
    else
      printf '%s\n' "$line"
    fi
  done <"$1"
  # Keep trailing blank lines significant through command substitution.
  printf '%s' '<end-of-unit>'
}
expected_unit_file="$(mktemp)"
printf '%s\n' "${DSH_OCEAN_OLD_UNIT%$'\n'}" >"$expected_unit_file"
if ! actual_fingerprint="$(unit_fingerprint "$UNIT")" ||
   ! expected_fingerprint="$(unit_fingerprint "$expected_unit_file")" ||
   [ "$actual_fingerprint" != "$expected_fingerprint" ]; then
  rm -f -- "$expected_unit_file"
  die 'unit has custom changes; automatic update refused'
fi
rm -f -- "$expected_unit_file"
fragment="$(systemctl --user show "$SERVICE_UNIT" --property=FragmentPath --value)" || die 'could not inspect unit fragment'
[ "$(readlink -f -- "$fragment")" = "$UNIT" ] || die 'active unit is not the managed unit file'
drop_ins="$(systemctl --user show "$SERVICE_UNIT" --property=DropInPaths --value)" || die 'could not inspect unit drop-ins'
[ -z "$drop_ins" ] || die 'custom unit drop-ins are not supported by automatic updates'
systemctl --user is-active --quiet "$SERVICE_UNIT" || die 'managed service is not active'

health() {
  local root="$1" helper
  if [ -n "${DSH_OCEAN_SERVICE_HEALTH_JS:-}" ]; then
    "$root/runtime/node/bin/node" --input-type=module -e "$DSH_OCEAN_SERVICE_HEALTH_JS" "$root"
  else
    helper="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)/check-service.mjs"
    "$root/runtime/node/bin/node" "$helper" "$root"
  fi
}
export PATH="$OLD_ROOT/runtime/node/bin:$PATH"
# Do not mutate anything if the current server is already unhealthy or mismatched.
health "$OLD_ROOT"

TXN=''
UNIT_STAGE=''
UNIT_CHANGED=0
NEEDS_ROLLBACK=0
COMMITTED=0
select_old() {
  [ -d "$OLD_ROOT" ] || return 1
  ln -s -- "$OLD_TARGET" "$TXN/old-selector" || return 1
  mv -Tf -- "$TXN/old-selector" "$PREFIX/current"
}
cleanup() {
  local status=$? failed=0
  trap - EXIT INT TERM
  set +e
  if [ "$COMMITTED" != 1 ] && [ "$NEEDS_ROLLBACK" = 1 ]; then
    log 'activation failed; restoring previous generation and service unit'
    select_old || failed=1
    if [ "$UNIT_CHANGED" = 1 ]; then
      if [ -f "$UNIT" ] && [ ! -L "$UNIT" ] && \
         { [ "$(<"$UNIT")" = "${content%$'\n'}" ] || [ "$(<"$UNIT")" = "$(<"$TXN/unit.before")" ]; }; then
        cp -p -- "$TXN/unit.before" "$UNIT_STAGE/restore" && mv -Tf -- "$UNIT_STAGE/restore" "$UNIT" || failed=1
        systemctl --user daemon-reload || failed=1
        systemctl --user restart "$SERVICE_UNIT" || failed=1
        health "$OLD_ROOT" || failed=1
      else
        log 'unit changed outside this transaction; preserving edits for manual recovery' >&2
        failed=1
      fi
    fi
    if [ "$failed" = 0 ]; then
      log 'rollback complete; previous service restored'
    else
      log "ERROR: rollback incomplete; recovery snapshot retained at $TXN" >&2
      status=1
    fi
  fi
  case "$UNIT_STAGE" in "$UNIT_DIR/.dsh-ocean-update."*) rm -rf -- "$UNIT_STAGE" ;; esac
  if [ "$failed" = 0 ]; then
    case "$TXN" in "$PREFIX/.update."*) rm -rf -- "$TXN" ;; esac
  fi
  exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

TXN="$(mktemp -d "$PREFIX/.update.XXXXXX")"
UNIT_STAGE="$(mktemp -d "$UNIT_DIR/.dsh-ocean-update.XXXXXX")"
cp -p -- "$UNIT" "$TXN/unit.before"
NEEDS_ROLLBACK=1
log 'installing and validating a separate runtime generation'
if [ -n "${DSH_OCEAN_INSTALL_SCRIPT:-}" ]; then
  /bin/bash -s <<<"$DSH_OCEAN_INSTALL_SCRIPT"
else
  /bin/bash "$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)/bootstrap-runtime.sh"
fi
NEW_TARGET="$(readlink "$PREFIX/current")"
NEW_ROOT="$(root_for "$NEW_TARGET")"
[ "$NEW_TARGET" != "$OLD_TARGET" ] || die 'installer did not select a new generation'
[ "$(<"$NEW_ROOT/.previous")" = "$OLD_TARGET" ] || die 'candidate predecessor does not match transaction'
[[ "${DSH_OCEAN_NEW_UNIT:?unit template required}" == *@@DSH_OCEAN_GENERATION@@* ]] || die 'missing generation unit placeholder'
content="${DSH_OCEAN_NEW_UNIT//@@DSH_OCEAN_GENERATION@@/${NEW_TARGET##*/}}"
printf '%s' "$content" >"$UNIT_STAGE/candidate"
chmod --reference="$UNIT" "$UNIT_STAGE/candidate"
# Do not overwrite edits made while the network installation was running.
[ -f "$UNIT" ] && [ ! -L "$UNIT" ] || die 'unit path changed during installation'
[ "$(<"$UNIT")" = "$(<"$TXN/unit.before")" ] || die 'unit changed during installation; activation refused'
drop_ins="$(systemctl --user show "$SERVICE_UNIT" --property=DropInPaths --value)" || die 'could not recheck unit drop-ins'
[ -z "$drop_ins" ] || die 'unit drop-ins changed during installation; activation refused'
# The rollback guard precedes replacement, including cancellation at this point.
UNIT_CHANGED=1
mv -Tf -- "$UNIT_STAGE/candidate" "$UNIT"
systemctl --user daemon-reload
log 'restarting the managed service using the verified generation'
systemctl --user restart "$SERVICE_UNIT"
health "$NEW_ROOT"
COMMITTED=1
log 'update committed; previous generation retained for recovery'
