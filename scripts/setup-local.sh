#!/usr/bin/env bash
# Generate isolated local credentials; never reuse these certificates for deployment.
set -euo pipefail
umask 077
project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
demo_dir="$project_dir/.local/demo"
if [ -e "$demo_dir" ]; then
    echo "Refusing to overwrite $demo_dir; choose a fresh workspace or move the existing demo." >&2
    exit 1
fi
mkdir -p "$demo_dir"
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes \
    -keyout "$demo_dir/ca.key" -out "$demo_dir/ca.crt" -days 30 \
    -subj /CN=mynofy-local-test-ca -addext basicConstraints=critical,CA:TRUE 2>/dev/null
openssl req -new -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes \
    -keyout "$demo_dir/server.key" -out "$demo_dir/server.csr" -subj /CN=localhost 2>/dev/null
cat > "$demo_dir/server.ext" <<'EXT'
basicConstraints=critical,CA:FALSE
subjectAltName=DNS:localhost,IP:127.0.0.1
extendedKeyUsage=serverAuth
keyUsage=critical,digitalSignature
EXT
openssl x509 -req -in "$demo_dir/server.csr" -CA "$demo_dir/ca.crt" -CAkey "$demo_dir/ca.key" \
    -CAcreateserial -out "$demo_dir/server.crt" -days 30 -extfile "$demo_dir/server.ext" 2>/dev/null
cd "$project_dir"
cargo run --quiet -- keygen --out "$demo_dir/client.pk8" > "$demo_dir/client-public.json"
python3 - "$project_dir" "$demo_dir" <<'PY'
import json,sys
from datetime import datetime,timedelta,timezone
from pathlib import Path
root,demo=map(Path,sys.argv[1:])
cfg=json.loads((root/'config/scenarios/local.json').read_text())
pub=json.loads((demo/'client-public.json').read_text())
cfg['service']['tls']={'certificate_file':'server.crt','private_key_file':'server.key','peer_ca_file':'ca.crt'}
cfg['service']['admin_socket']='run/mynofy-admin.sock'
cfg['clients']=[{'client_id':pub['client_id'],'display_name':'local-demo','keys':[{'algorithm':'ed25519','public_key':pub['public_key'],'valid_until':(datetime.now(timezone.utc)+timedelta(days=30)).isoformat(timespec='seconds').replace('+00:00','Z')}]}]
for topic in cfg['topics']:
    topic['acl']=[{'client_id':pub['client_id'],'allow':['publish','subscribe','inspect_own']}]
cfg['security_storage']={'directory':'security'}
(demo/'config-seed.json').write_text(json.dumps(cfg,indent=2)+'\n')
PY
cargo run --quiet --locked -- config-convert --config "$demo_dir/config-seed.json" --out "$demo_dir/config.yaml"
cargo run --quiet --locked -- security-init --config "$demo_dir/config.yaml"
rm -- "$demo_dir/config-seed.json"
printf 'Local scenario created in %s\n' "$demo_dir"
printf 'Start: cargo run -- serve --config .local/demo/config.yaml\n'
printf 'Client: cargo run -- client --url wss://localhost:8080 --ca .local/demo/ca.crt --key .local/demo/client.pk8 --owner local\n'
