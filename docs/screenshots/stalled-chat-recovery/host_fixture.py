#!/usr/bin/env python3
"""Synthetic stdio host: drop chat replies on its first connection only."""
import fcntl
import json
import os
import pathlib
import sys
import time

if '--version' in sys.argv:
    print('xd-host 0.2.8 (chat-recovery-fixture)')
    sys.exit(0)

root = pathlib.Path(os.environ['XD_RECOVERY_PROOF'])
root.mkdir(parents=True, exist_ok=True)
with (root / 'connections').open('a+') as state:
    fcntl.flock(state, fcntl.LOCK_EX)
    state.seek(0)
    attempt = int(state.read() or '0') + 1
    state.seek(0)
    state.truncate()
    state.write(str(attempt))
    state.flush()

chat = 'chat-proof'
tree = {
    'ok': True,
    'folders': [{'id': 'project-proof', 'name': 'Reconnect recovery'}],
    'chats': [{'id': chat, 'folder': 'project-proof', 'title': 'Recover after host stalls',
               'backend': 'codex', 'working': False, 'session': 'session-proof'}],
    'sessions': [{'id': 'session-proof', 'folder': 'project-proof', 'title': 'Recover after host stalls',
                  'chats': [chat], 'working': False, 'terminal_working': False,
                  'terminal_chat': 'session:session-proof'}],
}
for line in sys.stdin:
    try:
        req = json.loads(line)
        op = req.get('op')
        with (root / 'requests.jsonl').open('a') as log:
            log.write(json.dumps({'connection': attempt, 'op': op, 'at': time.time()}) + '\n')
        if attempt == 1 and op in ('chat', 'messages'):
            continue
        body = {'ok': True}
        if op == 'tree':
            body = tree.copy()
        elif op == 'chat':
            body.update(title='Recover after host stalls', backend='codex', session_mode='chat',
                        session='session-proof', session_title='Recover after host stalls',
                        terminal_chat='session:session-proof', workdir=str(root),
                        auth_state='authenticated', commands=[], plan=False, queue=[],
                        working=False, background_tasks=0, resumable=False, effort='medium',
                        new_worktree=False, has_messages=True, access='full',
                        draft='My unsent message is still here.', draft_revision=0, draft_attachments=[])
        elif op == 'messages':
            body.update(messages=[
                {'id': 1, 'role': 'user', 'content': 'The host restarted while this chat was opening.', 'at': 1794190000},
                {'id': 2, 'role': 'assistant', 'content': 'The chat recovered automatically. Your draft is ready to send.', 'at': 1794190001}
            ], total_messages=2, last_message_id=2, offset=0, has_older=False, has_newer=False)
        elif op == 'agent-catalog':
            body.update(backends=[{'id': 'codex', 'name': 'Codex', 'models': [], 'efforts': ['medium', 'high']}])
        elif op == 'terminal-list':
            body['terminals'] = []
        elif op == 'file-list':
            body.update(entries=[], path=req.get('path', ''))
        elif op == 'send':
            (root / 'submitted.txt').write_text(req.get('text', ''))
        body['_xd_request'] = req.get('_xd_request')
        print(json.dumps(body), flush=True)
    except (ValueError, BrokenPipeError):
        break
