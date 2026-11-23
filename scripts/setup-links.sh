#!/usr/bin/env bash
# Isolated three-service CA/mTLS demo; run as the intended unprivileged service user.
set -euo pipefail
umask 077
project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
demo_dir=${MYNOFY_LINKS_DIR:-"$project_dir/.local/links"}
if [ -e "$demo_dir" ]; then
    echo "Refusing to overwrite $demo_dir" >&2
    exit 1
fi
if [ -z "${MYNOFY_BINARY:-}" ]; then
    cd "$project_dir"
    cargo build --quiet --locked -p mynofy-cli
    mynofy_bin="$project_dir/target/debug/mynofy"
else
    mynofy_bin="$MYNOFY_BINARY"
fi
mkdir -p "$demo_dir/security"
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes \
    -keyout "$demo_dir/ca.key" -out "$demo_dir/ca.crt" -days 30 \
    -subj /CN=mynofy-links-demo -addext basicConstraints=critical,CA:TRUE 2>/dev/null
for node in edge hub owner; do
    openssl req -new -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes \
        -keyout "$demo_dir/$node.key" -out "$demo_dir/$node.csr" -subj "/CN=$node" 2>/dev/null
    cat > "$demo_dir/$node.ext" <<EXT
basicConstraints=critical,CA:FALSE
subjectAltName=DNS:$node,DNS:localhost,IP:127.0.0.1
extendedKeyUsage=serverAuth,clientAuth
keyUsage=critical,digitalSignature
EXT
    openssl x509 -req -in "$demo_dir/$node.csr" -CA "$demo_dir/ca.crt" -CAkey "$demo_dir/ca.key" \
        -CAcreateserial -out "$demo_dir/$node.crt" -days 30 -extfile "$demo_dir/$node.ext" 2>/dev/null
done
"$mynofy_bin" keygen --out "$demo_dir/client.pk8" > "$demo_dir/client-public.json"
python3 - "$project_dir" "$demo_dir" <<'PY'
import json,sys
from datetime import datetime,timedelta,timezone
from pathlib import Path
root,demo=map(Path,sys.argv[1:]);demo=demo.resolve()
base=json.loads((root/'config/scenarios/local.json').read_text())
pub=json.loads((demo/'client-public.json').read_text())
client={'client_id':pub['client_id'],'keys':[{'algorithm':'ed25519','public_key':pub['public_key'],'valid_until':(datetime.now(timezone.utc)+timedelta(days=30)).isoformat(timespec='seconds').replace('+00:00','Z')}]}
acl=[{'client_id':pub['client_id'],'allow':['publish','subscribe','inspect_own']}]
policy={'default_ttl_seconds':300,'max_ttl_seconds':3600}
names=['edge','hub','owner']
for i,name in enumerate(names):
    cfg=json.loads(json.dumps(base))
    cfg['service']={'id':name,'listen':f'127.0.0.1:{8090+i}','admin_socket':str(demo/f'run/{name}-admin.sock'),'tls':{'certificate_file':str(demo/f'{name}.crt'),'private_key_file':str(demo/f'{name}.key'),'peer_ca_file':str(demo/'ca.crt')}}
    cfg['security_storage']={'directory':str(demo/f'security/{name}')}
    cfg['clients']=[client]
    cfg['topics']=[{'name':f'{name}/events','owner_id':name,'policy':policy,'acl':acl}]
    if i<2:
        cfg['upstream']={'service_id':names[i+1],'url':f'wss://127.0.0.1:{8091+i}/','trust':{'mode':'ca','ca_file':str(demo/'ca.crt'),'expected_certificate_identity':names[i+1]}}
        cfg['topics'].append({'name':'team/events','owner_id':'owner','via':names[i+1]})
    else:
        cfg['topics'].append({'name':'team/events','owner_id':'owner','policy':policy,'acl':acl})
    if i>0:cfg['accepted_peers']=[{'service_id':names[i-1],'topics':['team/events']}]
    (demo/f'{name}-seed.json').write_text(json.dumps(cfg,indent=2)+'\n')
PY
for node in edge hub owner; do
    "$mynofy_bin" config-convert --config "$demo_dir/$node-seed.json" --out "$demo_dir/$node.yaml"
    rm -- "$demo_dir/$node-seed.json"
done
printf 'Created %s; start owner.yaml, hub.yaml, edge.yaml in separate terminals. Ports: 8092, 8091, 8090.\n' "$demo_dir"
printf 'See docs/service-network.md for the end-to-end client command.\n'
