#!/usr/bin/env python3
"""Small local demo driver; stdlib only. Business decisions stay with agents."""
import argparse
import json
import os
from pathlib import Path
import shlex
import signal
import subprocess
import sys
import time
import uuid

ROOT = Path(__file__).resolve().parents[2]
VARIANT = os.environ.get('MYNOFY_DEMO_VARIANT', '1')
if VARIANT not in ('1', '2'):
    raise ValueError('MYNOFY_DEMO_VARIANT must be 1 or 2')
DEMO_NAME = 'agent-demo' if VARIANT == '1' else 'agent-demo-2'
STATE = Path(os.environ.get('MYNOFY_DEMO_DIR', ROOT / '.local' / DEMO_NAME)).resolve()
BIN = Path(os.environ.get('MYNOFY_BIN', ROOT / 'target/debug/mynofy')).resolve()
OWNER = DEMO_NAME
TOPIC_PREFIX = 'demo' if VARIANT == '1' else 'demo2'
TOPICS = [TOPIC_PREFIX + '/' + suffix for suffix in ['human', 'operator', 'agents']]
ROLES = ['human', 'operator', 'worker-1', 'worker-2', 'observer']


def command(role, mode='client'):
    return [str(BIN), mode, '--socket', str(STATE / 'run/mynofy.sock'),
            '--key', str(STATE / (role + '.pk8')), '--owner', OWNER]


def setup():
    if STATE.exists():
        raise RuntimeError(f'{STATE} already exists; reuse with start. No files overwritten.')
    STATE.mkdir(parents=True, mode=0o700)
    cfg = json.loads((ROOT / 'config/scenarios/unix.json').read_text())
    cfg['service'] = {'id': OWNER, 'unix_socket': 'run/mynofy.sock',
                      'admin_socket': 'run/admin.sock'}
    cfg['security_storage'] = {'directory': 'security'}
    cfg['content_storage'] = {'directory': 'content', 'encryption': 'none',
                              'max_snapshot_bytes': 134217728}
    cfg['clients'] = []
    clients = {}
    for role in ROLES:
        public = json.loads(subprocess.check_output(
            [str(BIN), 'keygen', '--out', str(STATE / (role + '.pk8'))], text=True))
        clients[role] = public['client_id']
        cfg['clients'].append({'client_id': public['client_id'], 'display_name': role,
                              'keys': [{'algorithm': 'ed25519',
                                        'public_key': public['public_key'],
                                        'valid_until': '2035-01-01T00:00:00Z'}]})
    cfg['topics'] = []
    for topic in TOPICS:
        acl = []
        for role in ROLES:
            allow = ['subscribe']
            if (role == 'human' or
                (role == 'operator' and topic != TOPICS[0]) or
                (role.startswith('worker-') and topic == TOPICS[2])):
                allow += ['publish', 'inspect_own']
            acl.append({'client_id': clients[role], 'allow': allow})
        cfg['topics'].append({'name': topic, 'owner_id': OWNER,
                              'policy': {'default_ttl_seconds': 86400,
                                         'max_ttl_seconds': 604800},
                              'storage': {'mode': 'interval', 'seconds': 1,
                                          'encrypted': False}, 'acl': acl})
    seed = STATE / 'seed.json'
    seed.write_text(json.dumps(cfg))
    subprocess.run([str(BIN), 'config-convert', '--config', str(seed),
                    '--out', str(STATE / 'config.yaml')], check=True)
    seed.unlink()
    subprocess.run([str(BIN), 'check-config', '--config', str(STATE / 'config.yaml')], check=True)
    (STATE / 'work').mkdir()
    (STATE / 'agent-state').mkdir()
    template = 'agent-demo' if VARIANT == '1' else 'agent-demo-2'
    prompts = (ROOT / 'examples' / template / 'agent-commands.md').read_text()
    prompts = prompts.replace('@ROOT@', str(ROOT)).replace('@STATE@', str(STATE))
    for role in ['operator', 'worker-1', 'worker-2']:
        prompts = prompts.replace('@MCP_' + role.upper().replace('-', '_') + '@',
                                  shlex.join(command(role, 'mcp')))
    (STATE / 'AGENT-COMMANDS.md').write_text(prompts)
    if VARIANT == '2':
        common, sections = prompts.split('## Operator – vollständiger Rollenstart', 1)
        operator, sections = sections.split('## Worker 1 – vollständiger Rollenstart', 1)
        worker1, sections = sections.split('## Worker 2 – vollständiger Rollenstart', 1)
        worker2, _ = sections.split('## Mensch – Auftrag und Bestätigung', 1)
        for name, section in [('operator', operator), ('worker-1', worker1), ('worker-2', worker2)]:
            (STATE / (name + '-START.md')).write_text(common + '\n## Deine Rolle\n' + section)
    print(f'Prepared: {STATE}\nAgent instructions: {STATE / "AGENT-COMMANDS.md"}')
    return clients


class Client:
    def __init__(self, role):
        self.proc = subprocess.Popen(command(role), stdin=subprocess.PIPE,
                                     stdout=subprocess.PIPE, text=True, bufsize=1)

    def request(self, kind, **fields):
        request = dict(version=1, type=kind, id=uuid.uuid4().hex, **fields)
        self.proc.stdin.write(json.dumps(request) + '\n')
        self.proc.stdin.flush()
        line = self.proc.stdout.readline()
        if not line:
            raise RuntimeError('Client disconnected. Restart this command after service restart.')
        result = json.loads(line)
        if result.get('type') == 'error':
            raise RuntimeError(json.dumps(result))
        return result

    def close(self):
        try:
            self.proc.stdin.close()
        except BrokenPipeError:
            pass
        try:
            self.proc.wait(timeout=3)
        except subprocess.TimeoutExpired:
            self.proc.terminate()
            self.proc.wait(timeout=3)
        finally:
            self.proc.stdout.close()


def human(text):
    client = Client('human')
    try:
        def send(value):
            if len(json.dumps({'text': value}, ensure_ascii=False).encode()) > 4096:
                raise RuntimeError('Message exceeds demo payload limit of 4096 bytes.')
            response = client.request('publish', topic=TOPICS[0], payload={'text': value})
            print(json.dumps(response, ensure_ascii=False), flush=True)
        if text:
            send(' '.join(text))
        else:
            print('Human topic: one message per line, Ctrl-D to quit.', file=sys.stderr)
            for line in sys.stdin:
                if line.strip():
                    send(line.rstrip('\n'))
    finally:
        client.close()


def watch(interval):
    if not 0.1 <= interval <= 60:
        raise RuntimeError('--interval must be between 0.1 and 60 seconds')
    client = Client('observer')
    cursors = {}
    print('Reading all active messages, then new messages. Ctrl-C to quit.', flush=True)
    try:
        while True:
            for topic in TOPICS:
                while True:
                    args = dict(topic=topic, limit=100)
                    if cursors.get(topic):
                        args['after'] = cursors[topic]
                    result = client.request('poll', **args)
                    for item in result.get('items', []):
                        msg = item['message']
                        payload = msg['payload']
                        value = payload.get('text') if isinstance(payload, dict) else None
                        print(f'[{topic}] [{msg["producer_id"]}] {value if value is not None else json.dumps(payload, ensure_ascii=False)}',
                              flush=True)
                    cursors[topic] = result.get('next_cursor') or cursors.get(topic)
                    if not result.get('has_more'):
                        break
            time.sleep(interval)
    finally:
        client.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='mode', required=True)
    for name in ['setup', 'start', 'save', 'status']:
        sub.add_parser(name)
    p = sub.add_parser('human'); p.add_argument('text', nargs='*')
    p = sub.add_parser('watch'); p.add_argument('--interval', type=float, default=1)
    p = sub.add_parser('client'); p.add_argument('role', choices=ROLES)
    p = sub.add_parser('mcp'); p.add_argument('role', choices=ROLES)
    args = parser.parse_args()
    if os.getuid() == 0 or os.getgid() == 0:
        raise RuntimeError('Run this demo as a normal user, not root (service privilege policy).')
    os.umask(0o077)
    if args.mode == 'setup':
        setup()
    elif args.mode == 'start':
        os.execv(str(BIN), [str(BIN), 'serve', '--config', str(STATE / 'config.yaml')])
    elif args.mode in ['save', 'status']:
        action = ['content', 'save'] if args.mode == 'save' else ['status']
        os.execv(str(BIN), [str(BIN), 'admin', *action, '--socket', str(STATE / 'run/admin.sock')])
    elif args.mode in ['client', 'mcp']:
        os.execv(str(BIN), command(args.role, args.mode))
    elif args.mode == 'human':
        human(args.text)
    else:
        watch(args.interval)


if __name__ == '__main__':
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(0))
    try:
        main()
    except KeyboardInterrupt:
        pass
    except (RuntimeError, OSError, ValueError, subprocess.SubprocessError) as error:
        print(f'agent-demo: {error}', file=sys.stderr)
        sys.exit(1)
