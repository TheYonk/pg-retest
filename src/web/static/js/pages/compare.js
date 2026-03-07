// Compare page
function comparePage() {
    return {
        workloads: [],
        runs: [],
        report: null,
        loading: true,

        async load() {
            const el = document.getElementById('compare-content');
            if (!el) return;
            el.innerHTML = Status.loading();

            const [wklRes, runsRes] = await Promise.all([
                api.listWorkloads(),
                api.listRuns({ run_type: 'replay', limit: 50 }),
            ]);

            this.workloads = wklRes.workloads || [];
            this.runs = (runsRes.runs || []).filter(r => r.status === 'completed' && r.results_path);
            this.loading = false;
            this.render(el);
        },

        render(el) {
            const wklOptions = this.workloads.map(w =>
                `<option value="${w.id}">${w.name}</option>`
            ).join('');

            const runOptions = this.runs.map(r =>
                `<option value="${r.id}">${r.run_type} — ${Tables.formatTimestamp(r.started_at)} ${r.target_conn ? '(' + Tables.truncateSQL(r.target_conn, 30) + ')' : ''}</option>`
            ).join('');

            el.innerHTML = `
            <div class="fade-in space-y-4">
                <div class="card">
                    <h3 class="section-title mb-4">Compare Source vs Replay</h3>
                    <div class="grid grid-cols-3 gap-4 mb-4">
                        <div>
                            <label class="label">Source Workload</label>
                            <select class="input" id="compare-workload">
                                <option value="">Select workload...</option>
                                ${wklOptions}
                            </select>
                        </div>
                        <div>
                            <label class="label">Replay Run</label>
                            <select class="input" id="compare-run">
                                <option value="">Select run...</option>
                                ${runOptions}
                            </select>
                        </div>
                        <div>
                            <label class="label">Threshold %</label>
                            <input class="input" type="number" id="compare-threshold" value="20" min="1">
                        </div>
                    </div>
                    <button class="btn btn-primary" onclick="runCompare()">Compute Comparison</button>
                </div>

                <div id="compare-results" class="space-y-4"></div>
            </div>
            `;
        },
    };
}

async function runCompare() {
    const workloadId = document.getElementById('compare-workload').value;
    const runId = document.getElementById('compare-run').value;
    if (!workloadId || !runId) { window.showToast('Select workload and run', 'error'); return; }

    const resultsEl = document.getElementById('compare-results');
    resultsEl.innerHTML = Status.loading('Computing comparison...');

    const threshold = parseFloat(document.getElementById('compare-threshold').value) || 20.0;
    const res = await api.computeCompare({ workload_id: workloadId, run_id: runId, threshold });

    if (res.error) {
        resultsEl.innerHTML = Status.error(res.error);
        return;
    }

    const report = res.report;
    resultsEl.innerHTML = `
        <!-- Latency overview -->
        <div class="grid-stats">
            ${Status.statCard({ label: 'Source Avg', value: Tables.formatDuration(report.source_avg_latency_us), color: 'accent' })}
            ${Status.statCard({ label: 'Replay Avg', value: Tables.formatDuration(report.replay_avg_latency_us), color: 'amber' })}
            ${Status.statCard({ label: 'Errors', value: report.total_errors, color: report.total_errors > 0 ? 'danger' : 'accent' })}
            ${Status.statCard({ label: 'Regressions', value: report.regressions.length, color: report.regressions.length > 0 ? 'danger' : 'accent' })}
        </div>

        <!-- Latency chart -->
        <div class="card">
            <div class="chart-container tall">
                <canvas id="compare-latency-chart"></canvas>
            </div>
        </div>

        <!-- Latency detail table -->
        <div class="card">
            <h3 class="section-title mb-3">Latency Percentiles</h3>
            <table class="data-table">
                <thead><tr><th>Metric</th><th class="text-right">Source</th><th class="text-right">Replay</th><th class="text-right">Change</th></tr></thead>
                <tbody>
                    ${['p50', 'p95', 'p99', 'avg'].map(m => {
                        const src = report[`source_${m}_latency_us`];
                        const rep = report[`replay_${m}_latency_us`];
                        const change = src > 0 ? ((rep - src) / src * 100) : 0;
                        const changeClass = change > 10 ? 'text-danger' : change < -10 ? 'text-accent' : 'text-slate-400';
                        return `<tr>
                            <td class="text-slate-300">${m.toUpperCase()}</td>
                            <td class="text-right">${Tables.formatDuration(src)}</td>
                            <td class="text-right">${Tables.formatDuration(rep)}</td>
                            <td class="text-right ${changeClass}">${change >= 0 ? '+' : ''}${change.toFixed(1)}%</td>
                        </tr>`;
                    }).join('')}
                </tbody>
            </table>
        </div>

        ${report.regressions.length > 0 ? `
        <div class="card">
            <h3 class="section-title mb-3">Regressions (>${threshold}% slower)</h3>
            <div class="overflow-x-auto">
                ${Tables.renderTable('regression-table', [
                    { label: 'Query', render: r => `<span class="text-slate-300">${Tables.truncateSQL(r.sql, 80)}</span>` },
                    { label: 'Original', render: r => Tables.formatDuration(r.original_us), align: 'right' },
                    { label: 'Replay', render: r => Tables.formatDuration(r.replay_us), align: 'right' },
                    { label: 'Change', render: r => `<span class="text-danger font-semibold">+${r.change_pct.toFixed(1)}%</span>`, align: 'right' },
                ], report.regressions)}
            </div>
        </div>
        ` : ''}
    `;

    setTimeout(() => Charts.createLatencyChart('compare-latency-chart', report), 100);
}
