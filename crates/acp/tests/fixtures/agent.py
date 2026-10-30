#!/usr/bin/env python3
"""Deterministic ACP v1 stdio agent. No network or credentials."""
import json
import os
import signal
import sys
import time

remote = 'mock-session'
active = None
permission = None
callbacks = {}

def send(message):
    print(json.dumps(dict(jsonrpc='2.0', **message)), flush=True)

def result(id, value):
    send(dict(id=id, result=value))

def update(value):
    send(dict(method='session/update', params=dict(sessionId=remote, update=value)))

def text(value):
    update(dict(sessionUpdate='agent_message_chunk', content=dict(type='text', text=value)))

def finish(reason='end_turn'):
    global active
    result(active, dict(stopReason=reason))
    active = None

for line in sys.stdin:
    msg = json.loads(line)
    method, id, params = msg.get('method'), msg.get('id'), msg.get('params', {})
    if method == 'initialize':
        assert params['protocolVersion'] == 1
        assert params['clientCapabilities']['terminal'] is False
        assert params['clientCapabilities']['fs']['writeTextFile'] is False
        if os.getenv('PID_FILE'):
            with open(os.environ['PID_FILE'], 'w') as f:
                f.write(str(os.getpid()))
        if os.getenv('HANG_INIT'):
            time.sleep(60)
        result(id, dict(protocolVersion=1, agentCapabilities=dict(loadSession=os.getenv('NO_LOAD') != '1', mcpCapabilities=dict(http=os.getenv('MCP_HTTP') == '1'))))
    elif method in ('session/new', 'session/load'):
        if os.getenv('MCP_CAPTURE'):
            with open(os.environ['MCP_CAPTURE'], 'w') as f:
                json.dump(params['mcpServers'], f)
        else:
            assert params['mcpServers'] == []
        assert os.path.isabs(params['cwd'])
        if os.getenv('AUTH_REQUIRED'):
            send(dict(id=id, error=dict(code=-32000, message='Authentication required')))
        else:
            if method == 'session/load':
                assert params['sessionId'] == remote
                update(dict(sessionUpdate='user_message_chunk', content=dict(type='text', text='earlier')))
                text('reply')
            result(id, dict(sessionId=remote))
    elif method == 'session/set_model':
        result(id, {})
    elif method == 'session/prompt':
        assert active is None, 'overlapping prompt'
        assert params['sessionId'] == remote
        active = id
        prompt = params['prompt'][0]['text']
        if prompt == 'inspect-attachments':
            text(json.dumps(params['prompt']))
            finish()
        elif prompt == 'wait':
            text('waiting')
        elif prompt == 'fail':
            send(dict(id=id, error=dict(code=-32603, message='mock failure')))
            active = None
        elif prompt == 'stop-error':
            finish('error')
        elif prompt == 'exit':
            sys.exit(0)
        elif prompt == 'close-stdout':
            signal.signal(signal.SIGTERM, signal.SIG_IGN)
            os.close(1)
            time.sleep(60)
        elif prompt == 'callbacks':
            callbacks = {201: 'fs/read_text_file', 202: 'fs/write_text_file', 203: 'terminal/create'}
            for key, name in callbacks.items():
                send(dict(id=key, method=name, params=dict(sessionId=remote)))
        elif prompt.startswith('permission'):
            update(dict(sessionUpdate='tool_call', toolCallId='tool-1', title='Run printf', kind='execute', status='pending'))
            allow = 'allow_always' if prompt == 'permission-always' else 'allow_once'
            permission = 100
            send(dict(id=permission, method='session/request_permission', params=dict(
                sessionId=remote,
                toolCall=dict(toolCallId='tool-1', kind='execute', title='Run printf', rawInput=dict(command='printf acp')),
                options=[dict(optionId='yes', name='Allow', kind=allow), dict(optionId='no', name='Deny', kind='reject_once')]
            )))
        else:
            update(dict(sessionUpdate='agent_thought_chunk', content=dict(type='text', text='Thinking')))
            text('Hel')
            text('lo')
            update(dict(sessionUpdate='tool_call', toolCallId='edit-1', kind='edit', title='Edit', status='completed',
                        content=[dict(type='diff', path='/tmp/example.txt', oldText='old\n', newText='new\n')]))
            finish()
    elif method == 'session/cancel':
        if os.getenv('IGNORE_CANCEL'):
            continue
        if active is not None:
            finish('cancelled')
    elif method is None and id == permission:
        outcome = msg['result']['outcome']
        assert outcome['outcome'] in ('selected', 'cancelled')
        if active is not None:
            text(json.dumps(outcome, sort_keys=True))
            update(dict(sessionUpdate='tool_call_update', toolCallId='tool-1', status='completed'))
            finish()
        permission = None
    elif method is None and id in callbacks:
        assert msg['error']['code'] == -32601, msg
        del callbacks[id]
        if not callbacks:
            text('callbacks declined')
            finish()
    else:
        send(dict(id=id, error=dict(code=-32601, message='Unknown method')))
