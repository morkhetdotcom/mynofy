#!/usr/bin/env bash
# Local Unix-socket scenario: keypair authentication without TLS certificate setup.
set -euo pipefail
umask 077
case "$(uname -s)" in Linux|Darwin) ;; *) echo "Unix demo requires Linux or macOS." >&2; exit 1 ;; esac
project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
demo_dir="$project_dir/.local/unix"
if [ -e "$demo_dir" ]; then
    echo "Refusing to overwrite $demo_dir; use the existing configuration or move it first." >&2
    exit 1
fi
mkdir -p "$demo_dir"
cd "$project_dir"
cargo run --quiet --locked -- keygen --out "$demo_dir/client.pk8" > "$demo_dir/client-public.json"
python3 - "$project_dir" "$demo_dir" <<'PY'
import json,sys
from datetime import datetime,timedelta,timezone
from pathlib import Path
root,demo=map(Path,sys.argv[1:])
cfg=json.loads((root/'config/scenarios/unix.json').read_text())
pub=json.loads((demo/'client-public.json').read_text())
expiry=(datetime.now(timezone.utc)+timedelta(days=30)).isoformat(timespec='seconds').replace('+00:00','Z')
cfg['service']['admin_socket']='run/mynofy-admin.sock'
cfg['clients']=[{'client_id':pub['client_id'],'display_name':'unix-demo','keys':[{'algorithm':'ed25519','public_key':pub['public_key'],'valid_until':expiry}]}]
for topic in cfg['topics']:
    topic['acl']=[{'client_id':pub['client_id'],'allow':['publish','subscribe','inspect_own']}]
cfg['security_storage']={'directory':'security'}
(demo/'config-seed.json').write_text(json.dumps(cfg,indent=2)+'\n')
PY
cargo run --quiet --locked -- config-convert --config "$demo_dir/config-seed.json" --out "$demo_dir/config.yaml"
cargo run --quiet --locked -- security-init --config "$demo_dir/config.yaml"
rm -- "$demo_dir/config-seed.json"
printf 'Unix scenario created in %s\n' "$demo_dir"
printf 'Start: cargo run --locked -- serve --config .local/unix/config.yaml\n'
printf 'Client: cargo run --locked -- client --socket .local/unix/run/mynofy.sock --key .local/unix/client.pk8 --owner local\n'
