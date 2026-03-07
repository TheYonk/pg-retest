// History page
function historyPage() {
    return {
        runs: [],
        trends: [],
        loading: true,
        filterType: '',

        async load() {
            const el = document.getElementById('history-content');
            if (!el) return;
            el.innerHTML = Status.loading();

            const [runsRes, trendsRes] = await Promise.all([
                api.listRuns({ limit: 50 }),
                api.runTrends({ limit: 20 }),
            ]);

            this.runs = runsRes.runs || [];
            this.trends = trendsRes.trends || [];
            this.loading = false;
            this.render(el);
        },

        render(el) {
            el.innerHTML = `
            <div class="fade-in space-y-4">
                <!-- Trend chart -->
                <div class="card">
                    <h3 class="section-title mb-3">Latency Trend</h3>
                    ${this.trends.length > 0
                        ? '<div class="chart-container"><canvas id="history-trend-chart"></canvas></div>'
                        : Status.empty('No completed runs with reports yet')
                    }
                </div>

                <!-- Filter + table -->
                <div class="card">
                    <div class="section-header">
                        <h3 class="section-title">Run History</h3>
                        <div class="flex gap-2">
                            <select class="input" style="width: auto" id="history-filter" onchange="filterHistory(this.value)">
                                <option value="">All types</option>
                                <option value="replay">Replay</option>
                                <option value="ab">A/B Test</option>
                                <option value="pipeline">Pipeline</option>
                            </select>
                        </div>
                    </div>
                    <div class="overflow-x-auto" id="history-table-container">
                        ${this.renderTable(this.runs)}
                    </div>
                </div>

                <!-- Detail modal -->
                <div id="history-detail-modal" class="hidden">
                    <div class="modal-overlay" onclick="closeHistoryDetail()">
                        <div class="modal-content" style="max-width: 800px" onclick="event.stopPropagation()">
                            <div id="history-detail-content"></div>
                        </div>
                    </div>
                </div>
            </div>
            `;

            if (this.trends.length > 0) {
                setTimeout(() => Charts.createTrendChart('history-trend-chart', this.trends), 100);
            }
        },

        renderTable(runs) {
            if (runs.length === 0) return Status.empty('No runs found');
            const columns = [
                { label: 'Type', key: 'run_type' },
                { label: 'Status', render: r => Tables.statusBadge(r.status) },
                { label: 'Workload', render: r => r.workload_id ? r.workload_id.substring(0, 8) + '…' : '—' },
                { label: 'Target', render: r => r.target_conn ? Tables.truncateSQL(r.target_conn, 35) : '—' },
                { label: 'Mode', render: r => r.replay_mode || '—' },
                { label: 'Scale', render: r => r.scale ? `${r.scale}x` : '—' },
                { label: 'Exit', render: r => Tables.exitCodeBadge(r.exit_code) },
                { label: 'Started', render: r => Tables.formatTimestamp(r.started_at) },
                { label: '', render: r => `<button class="btn btn-secondary btn-sm" onclick="viewRunDetail('${r.id}')">Details</button>` },
            ];
            return Tables.renderTable('history-runs', columns, runs);
        },
    };
}

async function filterHistory(type) {
    const container = document.getElementById('history-table-container');
    container.innerHTML = Status.loading();
    const res = await api.listRuns(type ? { run_type: type, limit: 50 } : { limit: 50 });
    const page = Alpine.$data(document.querySelector('[x-data="historyPage()"]'));
    if (page) {
        container.innerHTML = page.renderTable(res.runs || []);
    }
}

async function viewRunDetail(id) {
    const modal = document.getElementById('history-detail-modal');
    const content = document.getElementById('history-detail-content');
    content.innerHTML = Status.loading();
    modal.classList.remove('hidden');

    const res = await api.getRun(id);
    if (!res.run) {
        content.innerHTML = Status.error('Run not found');
        return;
    }

    const run = res.run;
    const report = res.report;

    content.innerHTML = `
        <div class="space-y-4">
            <div class="flex items-center justify-between">
                <h3 class="text-base font-semibold">Run Details</h3>
                <button class="btn btn-secondary btn-sm" onclick="closeHistoryDetail()">Close</button>
            </div>

            <div class="grid grid-cols-3 gap-3">
                <div class="card">
                    <div class="text-xs text-slate-500 uppercase mb-1">Type</div>
                    <div class="font-mono text-sm">${run.run_type}</div>
                </div>
                <div class="card">
                    <div class="text-xs text-slate-500 uppercase mb-1">Status</div>
                    <div>${Tables.statusBadge(run.status)}</div>
                </div>
                <div class="card">
                    <div class="text-xs text-slate-500 uppercase mb-1">Exit Code</div>
                    <div>${Tables.exitCodeBadge(run.exit_code)}</div>
                </div>
            </div>

            ${run.target_conn ? `
            <div class="card">
                <div class="text-xs text-slate-500 uppercase mb-1">Target</div>
                <div class="font-mono text-xs break-all">${run.target_conn}</div>
            </div>
            ` : ''}

            ${run.error_message ? `
            <div class="card border-danger/30">
                <div class="text-xs text-slate-500 uppercase mb-1">Error</div>
                <div class="text-danger text-sm">${run.error_message}</div>
            </div>
            ` : ''}

            ${report ? `
            <div class="card">
                <h4 class="section-title mb-3">Report</h4>
                <div class="chart-container">
                    <canvas id="detail-latency-chart"></canvas>
                </div>
            </div>
            ` : ''}
        </div>
    `;

    if (report) {
        setTimeout(() => Charts.createLatencyChart('detail-latency-chart', report), 100);
    }
}

function closeHistoryDetail() {
    document.getElementById('history-detail-modal').classList.add('hidden');
}
