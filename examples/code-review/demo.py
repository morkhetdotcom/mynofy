#!/usr/bin/env python3
"""English code-review demo: real broker traffic, no model runtime required."""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import shlex
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]
# Reuse the existing tested transport/provisioning helper, not agent decisions.
os.environ['MYNOFY_DEMO_VARIANT'] = '1'
spec = importlib.util.spec_from_file_location('review_transport', ROOT / 'examples/agent-demo/demo.py')
demo = importlib.util.module_from_spec(spec)
spec.loader.exec_module(demo)
demo.STATE = Path(os.environ.get('MYNOFY_REVIEW_DIR', ROOT / '.local/code-review')).resolve()
demo.OWNER = 'code-review'
demo.TOPICS = ['review/human', 'review/operator', 'review/agents']
ROLES = {'human': 'human', 'operator': 'operator', 'codex': 'worker-1',
         'claude': 'worker-2', 'observer': 'observer'}


def setup():
    if demo.STATE.exists():
        raise RuntimeError('State directory already exists; reuse it or choose MYNOFY_REVIEW_DIR')
    if not demo.BIN.is_file():
        raise RuntimeError('Build the CLI first: cargo build --locked -p mynofy-cli')
    identities = demo.setup()
    (demo.STATE / "IDENTITIES.json").write_text(json.dumps({role: identities[key] for role, key in ROLES.items()}, indent=2)+"\n")
    text = Path(__file__).with_name('agent-prompts.md').read_text()
    text = text.replace('@ROOT@', str(ROOT)).replace('@STATE@', str(demo.STATE))
    for role in ('operator', 'codex', 'claude'):
        text = text.replace('@MCP_' + role.upper() + '@', shlex.join(demo.command(ROLES[role], 'mcp')))
    (demo.STATE / 'AGENT-COMMANDS.md').write_text(text)
    print('English role prompts:', demo.STATE / 'AGENT-COMMANDS.md')


def exchange():
    """Deterministic example transcript; it does not implement or review code."""
    protocol = json.loads(Path(__file__).with_name('protocol.json').read_text())
    catalog = json.loads((ROOT / 'protocol/catalogs/mynofy-v1.json').read_text())
    actions = {a['name']: a['id'] for a in catalog['actions']}
    statuses = {s['name']: s['id'] for s in catalog['statuses']}
    clients = {}
    published = {}
    identities = json.loads((demo.STATE / "IDENTITIES.json").read_text())
    try:
        for role in ROLES:
            clients[role] = demo.Client(ROLES[role])
        # The broker, rather than role claims in payload text, enforces the boundary.
        try:
            clients['codex'].request('publish', topic=demo.TOPICS[0], payload={'text': 'not permitted'})
        except RuntimeError:
            pass
        else:
            raise RuntimeError('Worker unexpectedly obtained human-topic publish rights')

        def send(role, topic, payload, label):
            response = clients[role].request('publish', topic=topic, payload=payload)
            published[response['message_id']] = identities[role]
            print(f'{role:8} -> {topic:16} {label}', flush=True)

        def text(role, topic, value, label):
            send(role, topic, {'text': json.dumps(value, separators=(',', ':'))}, label)

        text('human', demo.TOPICS[0], {'task': 'TASK-001', 'instruction': 'Build and review slugify; demo only'}, 'human request')
        text('operator', demo.TOPICS[1], {'mandate': 'M1', 'scope': 'one standard-library function and review'}, 'mandate proposal M1')
        text('human', demo.TOPICS[0], {'confirm_mandate': 'M1', 'operator': 'operator-001'}, 'mandate confirmation')
        for role in ('codex', 'claude'):
            text(role, demo.TOPICS[2], {'hello': role+'-001', 'capability': 'code' if role == 'codex' else 'review'}, 'introduction')
        text('operator', demo.TOPICS[2], {'protocol_proposal': protocol}, 'protocol proposal code-review/1')
        for role in ('codex', 'claude', 'operator'):
            text(role, demo.TOPICS[2], {'accept': 'code-review/1', 'actor': protocol['actors'][role]}, 'explicit protocol acceptance')

        def event(role, action, status):
            payload = {'kind': 'event', 'catalog': 1, 'catalog_version': 1,
                       'category': 12, 'action': actions[action], 'status': statuses[status],
                       'parameters': [{'u32': n} for n in (1, protocol['actors'][role], 1)]}
            send(role, demo.TOPICS[2], payload, f'JOB/{action}/{status} [ticket=1, actor={protocol["actors"][role]}, attempt=1]')

        for role in ('codex', 'claude'):
            text('operator', demo.TOPICS[2], {'assign': 'TASK-001', 'to': role, 'attempt': 1}, 'step assignment (scripted)')
            event(role, 'START', 'PENDING')
            event(role, 'ACQUIRE', 'SUCCESS')
            event(role, 'START', 'SUCCESS')
            text(role, demo.TOPICS[2], {'artifact': 'scripted demonstration; no source written', 'task': 'TASK-001'}, 'artifact reference (scripted)')
            event(role, 'COMPLETE', 'SUCCESS')
        event('operator', 'COMPLETE', 'SUCCESS')
        send('operator', demo.TOPICS[1], {'kind': 'event', 'catalog': 1, 'catalog_version': 1,
             'category': 12, 'action': actions['COMPLETE'], 'status': statuses['SUCCESS'],
             'parameters': [{'u32': n} for n in (1, 1, 1)]}, 'parent completion summary')
        spoof = clients['claude'].request('publish', topic=demo.TOPICS[2], payload={'actor_id': 2, 'producer_id': identities['codex']})
        own = clients['claude'].request('get_message', message_id=spoof['message_id'])['message']
        if own['producer_id'] != identities['claude']:
            raise RuntimeError('Payload claim overrode actual producer')
        print('Payload claims Codex; owner-stamped producer remains Claude. Reject the actor mismatch.')
        published[spoof['message_id']] = identities['claude']
        observed = set()
        for topic in demo.TOPICS:
            page = clients['observer'].request('poll', topic=topic, limit=100)
            for item in page['items']:
                message = item['message']
                if message['id'] in published and message['producer_id'] != published[message['id']]:
                    raise RuntimeError('Owner-stamped producer does not match authenticated sender')
                if message['id'] == spoof['message_id']:
                    if message['producer_id'] == identities['codex']:
                        raise RuntimeError('Spoofed actor was accepted as Codex')
                    print('Observer ignored the actor mismatch; business rejections use the agreed application protocol.')
                observed.add(message['id'])
        if not set(published).issubset(observed):
            raise RuntimeError('Observer did not receive every message')
        print(f'Observed all {len(published)} messages; worker human-topic publish was denied.')
        print('Scripted states only: no code was generated, reviewed, or approved by an AI.')
    finally:
        for client in clients.values():
            client.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='command', required=True)
    for name in ('setup', 'start', 'exchange'):
        sub.add_parser(name)
    sub.add_parser('watch').add_argument('--interval', type=float, default=1)
    sub.add_parser('human').add_argument('text', nargs='*')
    for name in ('client', 'mcp'):
        sub.add_parser(name).add_argument('role', choices=ROLES)
    args = parser.parse_args()
    if os.geteuid() == 0:
        parser.error('Run this local example as a normal user')
    if args.command == 'setup': setup()
    elif args.command == 'start':
        subprocess.run([str(demo.BIN), 'serve', '--config', str(demo.STATE / 'config.yaml')], check=True)
    elif args.command == 'watch': demo.watch(args.interval)
    elif args.command == 'human': demo.human(args.text)
    elif args.command == 'exchange': exchange()
    else:
        argv = demo.command(ROLES[args.role], args.command)
        os.execv(argv[0], argv)


if __name__ == '__main__':
    try:
        main()
    except (RuntimeError, subprocess.CalledProcessError) as error:
        raise SystemExit(str(error))
