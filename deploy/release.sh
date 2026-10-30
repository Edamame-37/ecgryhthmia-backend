#!/usr/bin/env bash
set -Eeuo pipefail
artifact=${1:?artifact required}
sha=${2:?commit SHA required}
[[ "$sha" =~ ^[0-9a-f]{40}$ ]] || exit 2
root=/opt/ecgrhythmia
exec 9>"$root/.release.lock"
flock -n 9 || { echo "Another deployment is running"; exit 2; }
release="$root/releases/$sha"
[[ ! -e "$release" ]] || { echo "Immutable release exists"; exit 2; }
mkdir -p "$release"

python3 - "$artifact" <<'PY'
import sys,tarfile,pathlib
with tarfile.open(sys.argv[1]) as archive:
 members=archive.getmembers()
 assert sum(member.size for member in members)<400*1024*1024, "Oversized artifact"
 for member in members:
  path=pathlib.PurePosixPath(member.name)
  assert not path.is_absolute() and '..' not in path.parts, "Unsafe artifact path"
  assert member.isfile() or member.isdir(), "Artifact links/devices prohibited"
  assert not member.mode & 0o7000, "Unsafe artifact permissions"
PY
tar -xzf "$artifact" -C "$release" --no-same-owner --no-same-permissions
(cd "$release"; sha256sum -c SHA256SUMS)
[[ "$(cat "$release/RELEASE_SHA")" == "$sha" ]]
previous=$(readlink -f "$root/current" || true)
ln -s "$release" "$root/current.next"
mv -Tf "$root/current.next" "$root/current"
restart_ok=true
if ! sudo systemctl restart ecg-backend; then restart_ok=false; fi
healthy=false
for attempt in {1..20}; do
 if [[ "$restart_ok" == true ]] && curl --max-time 5 --fail --silent http://127.0.0.1:8081/api/ready | python3 -c 'import json,sys; assert json.load(sys.stdin)["release"]==sys.argv[1]' "$sha"; then healthy=true; break; fi
 sleep 1
done
if [[ "$healthy" != true ]]; then
 restored=none
 if [[ -n "$previous" && "$previous" == "$root/releases/"* ]]; then
  ln -s "$previous" "$root/current.rollback"; mv -Tf "$root/current.rollback" "$root/current"; if ! sudo systemctl restart ecg-backend; then echo "Rollback restart failed" >&2; fi
  restored="$previous"
 fi
 echo "Readiness failed; restored binary: $restored" >&2
 exit 1
fi
echo "Released $sha"
