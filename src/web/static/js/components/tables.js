// Reusable table rendering helpers

const Tables = {
    renderHeader(columns) {
        return `<thead><tr>${columns.map(c =>
            `<th class="${c.align === 'right' ? 'text-right' : ''}">${c.label}</th>`
        ).join('')}</tr></thead>`;
    },

    renderRows(rows, columns, onClick) {
        if (!rows || rows.length === 0) {
            return `<tbody><tr><td colspan="${columns.length}" class="text-center text-slate-500 py-8">No data</td></tr></tbody>`;
        }
        return `<tbody>${rows.map(row => {
            const clickAttr = onClick ? `onclick="${onClick}('${row.id || ''}')" class="clickable"` : '';
            return `<tr ${clickAttr}>${columns.map(c => {
                let val = c.render ? c.render(row) : (row[c.key] ?? '—');
                const align = c.align === 'right' ? 'text-right' : '';
                return `<td class="${align}">${val}</td>`;
            }).join('')}</tr>`;
        }).join('')}</tbody>`;
    },

    renderTable(id, columns, rows, onClick) {
        return `<table class="data-table" id="${id}">
            ${this.renderHeader(columns)}
            ${this.renderRows(rows, columns, onClick)}
        </table>`;
    },

    statusBadge(status) {
        const map = {
            running: 'badge-info',
            completed: 'badge-success',
            failed: 'badge-danger',
            pending: 'badge-neutral',
            cancelled: 'badge-warning',
        };
        return `<span class="badge ${map[status] || 'badge-neutral'}">${status}</span>`;
    },

    exitCodeBadge(code) {
        if (code === null || code === undefined) return '—';
        if (code === 0) return '<span class="badge badge-success">PASS</span>';
        return `<span class="badge badge-danger">EXIT ${code}</span>`;
    },

    formatDuration(us) {
        if (!us) return '—';
        if (us < 1000) return `${us}µs`;
        if (us < 1_000_000) return `${(us / 1000).toFixed(1)}ms`;
        return `${(us / 1_000_000).toFixed(2)}s`;
    },

    formatTimestamp(ts) {
        if (!ts) return '—';
        const d = new Date(ts);
        return d.toLocaleString('en-US', {
            month: 'short', day: 'numeric',
            hour: '2-digit', minute: '2-digit',
        });
    },

    truncateSQL(sql, max = 80) {
        if (!sql) return '—';
        sql = sql.replace(/\s+/g, ' ').trim();
        return sql.length > max ? sql.substring(0, max) + '…' : sql;
    },
};
