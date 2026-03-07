// Form component helpers

const Forms = {
    connectionStringBuilder(prefix = '') {
        return `
        <div class="space-y-3">
            <div class="grid grid-cols-2 gap-3">
                <div>
                    <label class="label">Host</label>
                    <input class="input" type="text" placeholder="localhost"
                           x-model="${prefix}host" value="localhost">
                </div>
                <div>
                    <label class="label">Port</label>
                    <input class="input" type="text" placeholder="5432"
                           x-model="${prefix}port" value="5432">
                </div>
            </div>
            <div class="grid grid-cols-2 gap-3">
                <div>
                    <label class="label">Database</label>
                    <input class="input" type="text" placeholder="postgres"
                           x-model="${prefix}database">
                </div>
                <div>
                    <label class="label">User</label>
                    <input class="input" type="text" placeholder="postgres"
                           x-model="${prefix}user">
                </div>
            </div>
            <div>
                <label class="label">Password</label>
                <input class="input" type="password" placeholder="password"
                       x-model="${prefix}password">
            </div>
            <div>
                <label class="label">Connection String (auto-built or enter directly)</label>
                <input class="input" type="text"
                       placeholder="postgres://user:pass@host:5432/dbname"
                       x-model="${prefix}connString">
            </div>
        </div>`;
    },

    buildConnString(parts) {
        if (parts.connString) return parts.connString;
        const user = parts.user || 'postgres';
        const pass = parts.password ? `:${parts.password}` : '';
        const host = parts.host || 'localhost';
        const port = parts.port || '5432';
        const db = parts.database || 'postgres';
        return `postgres://${user}${pass}@${host}:${port}/${db}`;
    },

    select(label, model, options) {
        const opts = options.map(o =>
            `<option value="${o.value}">${o.label}</option>`
        ).join('');
        return `
        <div>
            <label class="label">${label}</label>
            <select class="input" x-model="${model}">${opts}</select>
        </div>`;
    },

    checkbox(label, model) {
        return `
        <label class="flex items-center gap-2 cursor-pointer text-sm text-slate-300">
            <input type="checkbox" x-model="${model}"
                   class="w-4 h-4 rounded border-slate-600 bg-slate-800 text-accent focus:ring-accent/30">
            ${label}
        </label>`;
    },

    slider(label, model, min, max, step = 1) {
        return `
        <div>
            <label class="label">${label}: <span class="text-accent" x-text="${model}"></span></label>
            <input type="range" min="${min}" max="${max}" step="${step}"
                   x-model="${model}"
                   class="w-full h-1 bg-slate-700 rounded-lg appearance-none cursor-pointer accent-teal-500">
        </div>`;
    },
};
