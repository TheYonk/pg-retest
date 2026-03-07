// Status display components

const Status = {
    statCard(opts) {
        const { label, value, sub, color = 'accent', icon = '' } = opts;
        const colorMap = {
            accent: '--glow-color: rgba(45, 212, 191, 0.5)',
            amber: '--glow-color: rgba(251, 191, 36, 0.5)',
            danger: '--glow-color: rgba(244, 63, 94, 0.5)',
            blue: '--glow-color: rgba(96, 165, 250, 0.5)',
        };
        return `
        <div class="card stat-card" style="${colorMap[color] || colorMap.accent}">
            <div class="flex items-center justify-between mb-2">
                <span class="text-xs font-medium text-slate-500 uppercase tracking-wider">${label}</span>
                ${icon ? `<span class="text-lg opacity-50">${icon}</span>` : ''}
            </div>
            <div class="text-2xl font-semibold font-mono text-${color === 'accent' ? 'accent' : color + '-400'}">${value}</div>
            ${sub ? `<div class="text-xs text-slate-500 mt-1">${sub}</div>` : ''}
        </div>`;
    },

    progressBar(pct, label = '') {
        return `
        <div>
            ${label ? `<div class="flex items-center justify-between mb-1">
                <span class="text-xs text-slate-400">${label}</span>
                <span class="text-xs font-mono text-accent">${pct.toFixed(1)}%</span>
            </div>` : ''}
            <div class="progress-bar">
                <div class="progress-bar-fill" style="width: ${pct}%"></div>
            </div>
        </div>`;
    },

    loading(text = 'Loading...') {
        return `
        <div class="flex items-center justify-center py-12 text-slate-500">
            <span class="spinner mr-3"></span>
            <span class="text-sm">${text}</span>
        </div>`;
    },

    empty(text = 'No data') {
        return `
        <div class="flex flex-col items-center justify-center py-12 text-slate-500">
            <svg class="w-12 h-12 mb-3 opacity-30" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5">
                <path d="M20 7l-8-4-8 4m16 0l-8 4m8-4v10l-8 4m0-10L4 7m8 4v10M4 7v10l8 4"/>
            </svg>
            <span class="text-sm">${text}</span>
        </div>`;
    },

    error(text) {
        return `
        <div class="card border-danger/30 bg-danger/5">
            <div class="flex items-center gap-2 text-danger text-sm">
                <svg class="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                    <circle cx="12" cy="12" r="10"/><line x1="15" y1="9" x2="9" y2="15"/><line x1="9" y1="9" x2="15" y2="15"/>
                </svg>
                ${text}
            </div>
        </div>`;
    },
};
