#!/usr/bin/env python3
"""Collects the Python code of an Ignition 8.3 project directory for the
differential test: every .py file, and every script embedded in a Perspective
view.json, wrapped in the function header Ignition compiles it with.

usage: extract_view_scripts.py PROJECT_DIR OUT_DIR

Writes OUT_DIR/000001.py, ... and OUT_DIR/list.txt. The output names carry no
information about the input, so that the results of private corpora can be
reported in aggregate only.
"""
import json
import os
import sys

HEADERS = {
    'event': 'def runAction(self, event):',
    'transform': 'def transform(self, value, quality, timestamp):',
    'change': 'def valueChanged(self, previousValue, currentValue, origin, missedEvents):',
    'message': 'def onMessageReceived(self, payload):',
}


def wrap(header, body):
    """Ignition indents the body by one tab under the generated header."""
    lines = body.replace('\r\n', '\n').split('\n')
    return header + '\n' + '\n'.join('\t' + line for line in lines) + '\n'


def view_scripts(node, path):
    """Yields (header, body) for every script in a parsed view.json."""
    if isinstance(node, dict):
        script = node.get('script')
        if isinstance(script, str):
            if 'customMethods' in path:
                params = node.get('params') or []
                name = node.get('name') or 'method'
                args = ', '.join(['self'] + [str(p) for p in params])
                yield 'def %s(%s):' % (name, args), script
            elif 'messageHandlers' in path:
                yield HEADERS['message'], script
            elif path and path[-1] == 'onChange':
                yield HEADERS['change'], script
            elif 'events' in path:
                yield HEADERS['event'], script
        code = node.get('code')
        if isinstance(code, str) and node.get('type') == 'script':
            yield HEADERS['transform'], code
        for key, value in node.items():
            yield from view_scripts(value, path + [key])
    elif isinstance(node, list):
        for item in node:
            yield from view_scripts(item, path)


def main():
    root, out = sys.argv[1], sys.argv[2]
    os.makedirs(out, exist_ok=True)
    names = []

    def emit(data):
        name = '%06d.py' % (len(names) + 1)
        with open(os.path.join(out, name), 'wb') as f:
            f.write(data)
        names.append(name)

    for dirpath, dirnames, filenames in os.walk(root):
        # Skip version control and nested worktrees of the same project.
        dirnames[:] = sorted(d for d in dirnames if d not in ('.git', '.claude'))
        for filename in sorted(filenames):
            path = os.path.join(dirpath, filename)
            if filename.endswith('.py'):
                with open(path, 'rb') as f:
                    emit(f.read())
            elif filename == 'view.json':
                try:
                    with open(path, encoding='utf-8') as f:
                        view = json.load(f)
                except (ValueError, UnicodeDecodeError):
                    continue
                for header, body in view_scripts(view, []):
                    emit(wrap(header, body).encode('utf-8'))
    with open(os.path.join(out, 'list.txt'), 'w') as f:
        f.write('\n'.join(names) + '\n')
    print('%d sources' % len(names))


if __name__ == '__main__':
    main()
