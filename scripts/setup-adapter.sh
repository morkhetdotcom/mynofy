#!/usr/bin/env bash
# Fresh Unix scenario with three provisioned workflow topics and an echo adapter.
set -euo pipefail
umask 077
case "$(uname -s)" in Linux|Darwin) ;; *) echo "Unix adapter demo requires Linux or macOS." >&2; exit 1 ;; esac
project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
demo_dir="$project_dir/.local/adapter"
if [ -e "$demo_dir" ]; then
    echo "Refusing to overwrite $demo_dir; use it or move it first." >&2
    exit 1
fi
mkdir -p "$demo_dir"
cd "$project_dir"
cargo run --quiet --locked -- keygen --out "$demo_dir/client.pk8" > "$demo_dir/client-public.json"
python3 - "$project_dir" "$demo_dir" <<'PY'
import json,sys,copy
from pathlib import Path
from datetime import datetime,timedelta,timezone
root,demo=map(Path,sys.argv[1:])
cfg=json.loads((root/'config/scenarios/unix.json').read_text())
key=json.loads((demo/'client-public.json').read_text())
expiry=(datetime.now(timezone.utc)+timedelta(days=30)).isoformat(timespec='seconds').replace('+00:00','Z')
cfg['clients']=[{'client_id':key['client_id'],'keys':[{'algorithm':'ed25519','public_key':key['public_key'],'valid_until':expiry}]}]
rule=json.loads((root/'config/adapters/echo.json').read_text())
template=cfg['topics'][0]
cfg['topics']=[]
for name in [rule['communication_topic'],rule['status_topic'],rule['interpretation_topic']]:
 topic=copy.deepcopy(template);topic['name']=name;topic['acl']=[{'client_id':key['client_id'],'allow':['publish','subscribe','inspect_own']}];cfg['topics'].append(topic)
cfg['security_storage']={'directory':'security'}
(demo/'config-seed.json').write_text(json.dumps(cfg,indent=2)+'\n')
(demo/'rules.json').write_text(json.dumps(rule,indent=2)+'\n')
PY
cargo run --quiet --locked -- config-convert --config "$demo_dir/config-seed.json" --out "$demo_dir/config.yaml"
rm -- "$demo_dir/config-seed.json"
printf 'Adapter scenario created in %s\n' "$demo_dir"
printf 'Server: cargo run --locked -- serve --config .local/adapter/config.yaml\n'
printf 'Adapter: cargo run --locked -- adapter --socket .local/adapter/run/mynofy.sock --key .local/adapter/client.pk8 --owner local --rules .local/adapter/rules.json\n'
