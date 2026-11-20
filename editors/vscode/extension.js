// Kestrel extension: "Run Kestrel" (ksc + ksvm in a terminal) and live errors.
const vscode = require('vscode');
const fs = require('fs');
const os = require('os');
const path = require('path');
const { execFile } = require('child_process');

const TERMINAL_NAME = 'Kestrel';

// Finds the folder holding ksc and ksvm. Returns '' to use the PATH.
function findBinDir(fileUri) {
    const configured = vscode.workspace.getConfiguration('kestrel').get('binDir');
    if (configured) {
        return configured;
    }
    const folder = vscode.workspace.getWorkspaceFolder(fileUri);
    if (folder) {
        for (const profile of ['release', 'debug']) {
            const dir = path.join(folder.uri.fsPath, 'target', profile);
            if (fs.existsSync(path.join(dir, 'ksc')) && fs.existsSync(path.join(dir, 'ksvm'))) {
                return dir;
            }
        }
    }
    return '';
}

// Wraps a path in single quotes so spaces and accents survive the shell.
function quote(text) {
    return "'" + text.replace(/'/g, "'\\''") + "'";
}

async function run(uri) {
    // From the explorer menu `uri` is the clicked file; otherwise the open editor.
    let document;
    if (uri instanceof vscode.Uri) {
        document = await vscode.workspace.openTextDocument(uri);
    } else if (vscode.window.activeTextEditor) {
        document = vscode.window.activeTextEditor.document;
    }
    if (!document || document.languageId !== 'kestrel') {
        vscode.window.showErrorMessage('Run Kestrel: open a .ks file first.');
        return;
    }
    if (document.isDirty) {
        await document.save();
    }

    const source = document.uri.fsPath;
    const output = source.replace(/\.ks$/, '') + '.ksb';
    const binDir = findBinDir(document.uri);
    const ksc = binDir ? quote(path.join(binDir, 'ksc')) : 'ksc';
    const ksvm = binDir ? quote(path.join(binDir, 'ksvm')) : 'ksvm';

    let terminal = vscode.window.terminals.find((t) => t.name === TERMINAL_NAME);
    if (!terminal) {
        terminal = vscode.window.createTerminal(TERMINAL_NAME);
    }
    terminal.show(true);
    terminal.sendText(`${ksc} ${quote(source)} -o ${quote(output)} && ${ksvm} ${quote(output)}`);
}

// ---------- Errors in the Problems panel ----------

// Parses `ksc` output: "error: msg", " --> file:line:col", "^^^", "= note: ...".
function parseDiagnostics(output) {
    const lines = output.split('\n');
    const result = [];
    for (let i = 0; i < lines.length; i++) {
        const head = lines[i].match(/^error: (.*)$/);
        if (!head) {
            continue;
        }
        let message = head[1];
        let line = 0;
        let col = 0;
        let width = 1;
        for (let j = i + 1; j < lines.length && !lines[j].startsWith('error: '); j++) {
            const at = lines[j].match(/-->\s.*:(\d+):(\d+)\s*$/);
            const carets = lines[j].match(/^\s*\|\s*(\^+)\s*$/);
            const note = lines[j].match(/^\s*= note: (.*)$/);
            if (at) {
                line = Number(at[1]) - 1;
                col = Number(at[2]) - 1;
            } else if (carets) {
                width = carets[1].length;
            } else if (note) {
                message += '\nnote: ' + note[1];
            }
        }
        const range = new vscode.Range(line, col, line, col + width);
        const diagnostic = new vscode.Diagnostic(range, message, vscode.DiagnosticSeverity.Error);
        diagnostic.source = 'ksc';
        result.push(diagnostic);
    }
    return result;
}

// Compiles a copy of the editor text (saved or not) to a temp folder and
// shows the errors. Each document keeps only its latest check.
function checkDocument(document, collection) {
    if (document.languageId !== 'kestrel' || document.uri.scheme !== 'file') {
        return;
    }
    const binDir = findBinDir(document.uri);
    const ksc = binDir ? path.join(binDir, 'ksc') : 'ksc';
    const tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), 'kestrel-'));
    const source = path.join(tmpDir, path.basename(document.uri.fsPath));
    fs.writeFileSync(source, document.getText());
    const version = document.version;

    execFile(ksc, [source, '-o', path.join(tmpDir, 'out.ksb')], (error, stdout, stderr) => {
        fs.rmSync(tmpDir, { recursive: true, force: true });
        if (document.isClosed || document.version !== version) {
            return; // the text changed meanwhile; a newer check is coming
        }
        if (error && error.code === 'ENOENT') {
            collection.delete(document.uri);
            return; // ksc not found: nothing to report
        }
        collection.set(document.uri, parseDiagnostics(stderr));
    });
}

// ---------- Outline (document symbols) ----------

// Blanks out strings, characters and comments, keeping every column in place,
// so braces and words inside them are not mistaken for code.
function blankNonCode(text) {
    let out = '';
    let i = 0;
    while (i < text.length) {
        const c = text[i];
        const next = text[i + 1];
        if (c === '/' && next === '/') {
            while (i < text.length && text[i] !== '\n') { out += ' '; i++; }
        } else if (c === '/' && next === '*') {
            const end = text.indexOf('*/', i + 2);
            const stop = end < 0 ? text.length : end + 2;
            for (; i < stop; i++) out += text[i] === '\n' ? '\n' : ' ';
        } else if (c === '"' || c === "'") {
            out += ' ';
            i++;
            while (i < text.length && text[i] !== c && text[i] !== '\n') {
                if (text[i] === '\\') { out += ' '; i++; }
                if (i < text.length) { out += ' '; i++; }
            }
            if (i < text.length && text[i] === c) { out += ' '; i++; }
        } else {
            out += c;
            i++;
        }
    }
    return out;
}

const K = vscode.SymbolKind;
const TYPE_KINDS = { struct: K.Struct, class: K.Class, interface: K.Interface, enum: K.Enum };

function provideSymbols(document) {
    const lines = blankNonCode(document.getText()).split('\n');
    const roots = [];
    const stack = [];         // open symbols: { symbol, open (brace depth), start (line, col) }
    let depth = 0;
    let pending = null;       // a symbol whose `{` has not been seen yet
    let impl = '';            // `@impl(...)` waiting for its class

    const add = (symbol) => (stack.length ? stack[stack.length - 1].symbol.children : roots).push(symbol);
    const make = (name, detail, kind, line, col, endCol) => {
        const sel = new vscode.Range(line, col, line, col + name.length);
        return new vscode.DocumentSymbol(name, detail, kind, new vscode.Range(line, 0, line, endCol), sel);
    };

    for (let ln = 0; ln < lines.length; ln++) {
        const text = lines[ln];
        const top = stack.length ? stack[stack.length - 1] : null;
        const inBody = top && depth === top.open + 1;
        let m;

        if ((m = text.match(/@impl\s*\(([^)]*)\)/))) {
            impl = 'impl ' + m[1].trim();
        }
        if ((m = text.match(/\b(struct|class|interface|enum)\s+([A-Za-z_]\w*)/))) {
            const kind = m[1] === 'class' && impl ? impl : '';
            const s = make(m[2], kind, TYPE_KINDS[m[1]], ln, m.index + m[0].length - m[2].length, text.length);
            add(s);
            pending = s;
            impl = '';
        } else if ((m = text.match(/\bproc\s+([A-Za-z_]\w*)\s*(\([^)]*\))?\s*(->\s*[^{;]+)?/))) {
            const inType = top && (top.symbol.kind === K.Class || top.symbol.kind === K.Interface);
            const detail = ((m[2] || '()') + ' ' + (m[3] || '')).replace(/\s+/g, ' ').trim();
            const s = make(m[1], detail, inType ? K.Method : K.Function, ln, m.index + m[0].indexOf(m[1], 4), text.length);
            add(s);
            if (!/;\s*$/.test(text)) pending = s;      // interface methods end in `;`
        } else if ((m = text.match(/^\s*const\s+([A-Za-z_]\w*)\s*:\s*([^=;]+)/))) {
            add(make(m[1], m[2].trim(), K.Constant, ln, text.indexOf(m[1], text.indexOf('const') + 5), text.length));
        } else if (inBody && (top.symbol.kind === K.Struct || top.symbol.kind === K.Class)
            && (m = text.match(/^\s*(?:public\s+|private\s+)?([A-Za-z_]\w*)\s*(\[\])?\s*:\s*([^;=]+);/))) {
            add(make(m[1] + (m[2] || ''), m[3].trim(), K.Field, ln, text.indexOf(m[1]), text.length));
        }

        for (let col = 0; col < text.length; col++) {
            if (text[col] === '{') {
                if (pending) {
                    stack.push({ symbol: pending, open: depth, start: [ln, col] });
                    pending = null;
                }
                depth++;
            } else if (text[col] === '}') {
                depth--;
                const open = stack[stack.length - 1];
                if (open && open.open === depth) {
                    stack.pop();
                    const s = open.symbol;
                    s.range = new vscode.Range(s.range.start, new vscode.Position(ln, col + 1));
                    if (s.kind === K.Enum) addEnumMembers(s, lines, open.start, [ln, col]);
                }
            }
        }
    }
    return roots;
}

// Enum values sit between the braces, separated by commas (`A, B = 5, C`).
function addEnumMembers(symbol, lines, start, end) {
    for (let ln = start[0]; ln <= end[0]; ln++) {
        const from = ln === start[0] ? start[1] + 1 : 0;
        const to = ln === end[0] ? end[1] : lines[ln].length;
        const re = /(?:^|,)\s*([A-Za-z_]\w*)/g;
        const part = lines[ln].slice(from, to);
        let m;
        while ((m = re.exec(part))) {
            const col = from + m.index + m[0].indexOf(m[1]);
            const r = new vscode.Range(ln, col, ln, col + m[1].length);
            symbol.children.push(new vscode.DocumentSymbol(m[1], '', K.EnumMember, r, r));
        }
    }
}

function activate(context) {
    context.subscriptions.push(vscode.commands.registerCommand('kestrel.run', run));
    context.subscriptions.push(vscode.languages.registerDocumentSymbolProvider(
        { language: 'kestrel' }, { provideDocumentSymbols: provideSymbols }));

    const collection = vscode.languages.createDiagnosticCollection('kestrel');
    context.subscriptions.push(collection);

    const timers = new Map();
    const schedule = (document) => {
        const key = document.uri.toString();
        clearTimeout(timers.get(key));
        timers.set(key, setTimeout(() => checkDocument(document, collection), 400));
    };

    vscode.workspace.textDocuments.forEach(schedule);
    context.subscriptions.push(
        vscode.workspace.onDidOpenTextDocument(schedule),
        vscode.workspace.onDidChangeTextDocument((e) => schedule(e.document)),
        vscode.workspace.onDidCloseTextDocument((d) => collection.delete(d.uri)),
    );
}

function deactivate() {}

module.exports = { activate, deactivate };
