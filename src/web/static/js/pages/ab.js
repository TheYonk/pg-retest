// A/B Testing page
function abPage() {
    return {
        workloads: [],
        loading: true,
        variants: [
            { label: 'Baseline', target: '' },
            { label: 'Variant B', target: '' },
        ],

        async load() {
            const el = document.getElementById('ab-content');
            if (!el) return;
            el.innerHTML = Status.loading();

            const res = await api.listWorkloads();
            this.workloads = res.workloads || [];
            this.loading = false;
            this.render(el);
        },

        render(el) {
            const wklOptions = this.workloads.map(w =>
                `<option value="${w.id}">${w.name} (${w.total_sessions}s / ${w.total_queries}q)</option>`
            ).join('');

            el.innerHTML = `
            <div class="fade-in space-y-4">
                <div class="card">
                    <h3 class="section-title mb-4">A/B Test Configuration</h3>
                    <div class="space-y-4">
                        <div class="grid grid-cols-2 gap-4">
                            <div>
                                <label class="label">Workload</label>
                                <select class="input" id="ab-workload">
                                    <option value="">Select workload...</option>
                                    ${wklOptions}
                                </select>
                            </div>
                            <div class="grid grid-cols-2 gap-3">
                                <div>
                                    <label class="label">Speed</label>
                                    <input class="input" type="number" id="ab-speed" value="1.0" step="0.1" min="0">
                                </div>
                                <div>
                                    <label class="label">Threshold %</label>
                                    <input class="input" type="number" id="ab-threshold" value="20" step="1" min="1">
                                </div>
                            </div>
                        </div>
                        <label class="flex items-center gap-2 cursor-pointer text-sm text-slate-300">
                            <input type="checkbox" id="ab-readonly"
                                   class="w-4 h-4 rounded border-slate-600 bg-slate-800">
                            Read-only mode
                        </label>
                    </div>
                </div>

                <!-- Variant definitions -->
                <div class="card">
                    <div class="section-header">
                        <h3 class="section-title">Variants</h3>
                        <button class="btn btn-secondary btn-sm" onclick="addABVariant()">+ Add Variant</button>
                    </div>
                    <div id="ab-variants" class="space-y-3"></div>
                </div>

                <div class="flex gap-2">
                    <button class="btn btn-primary" id="ab-start-btn" onclick="startABTest()">Start A/B Test</button>
                </div>

                <!-- Results -->
                <div id="ab-results" class="hidden space-y-4"></div>
            </div>
            `;

            this.renderVariants();
            this.setupWsListeners();
        },

        renderVariants() {
            const el = document.getElementById('ab-variants');
            if (!el) return;
            el.innerHTML = window._abVariants.map((v, i) => `
                <div class="grid grid-cols-12 gap-3 items-end">
                    <div class="col-span-3">
                        <label class="label">Label</label>
                        <input class="input" type="text" value="${v.label}"
                               onchange="window._abVariants[${i}].label = this.value"
                               placeholder="Variant name">
                    </div>
                    <div class="col-span-8">
                        <label class="label">Connection String</label>
                        <input class="input" type="text" value="${v.target}"
                               onchange="window._abVariants[${i}].target = this.value"
                               placeholder="postgres://user:pass@host:5432/dbname">
                    </div>
                    <div class="col-span-1">
                        ${i >= 2 ? `<button class="btn btn-danger btn-sm w-full" onclick="removeABVariant(${i})">×</button>` : ''}
                    </div>
                </div>
            `).join('');
        },

        setupWsListeners() {
            wsClient.on('ABVariantCompleted', (msg) => {
                window.showToast(`Variant "${msg.label}" completed`, 'info');
            });
            wsClient.on('ABCompleted', async (msg) => {
                window.showToast('A/B test completed!', 'success');
                document.getElementById('ab-start-btn').disabled = false;
                const res = await api.getAB(msg.run_id);
                if (res.report) renderABResults(res.report);
            });
        },
    };
}

// Global variant state
window._abVariants = [
    { label: 'Baseline', target: '' },
    { label: 'Variant B', target: '' },
];

function addABVariant() {
    const n = window._abVariants.length + 1;
    window._abVariants.push({ label: `Variant ${String.fromCharCode(64 + n)}`, target: '' });
    const page = Alpine.$data(document.querySelector('[x-data="abPage()"]'));
    if (page) page.renderVariants();
}

function removeABVariant(i) {
    window._abVariants.splice(i, 1);
    const page = Alpine.$data(document.querySelector('[x-data="abPage()"]'));
    if (page) page.renderVariants();
}

async function startABTest() {
    const workloadId = document.getElementById('ab-workload').value;
    if (!workloadId) { window.showToast('Select a workload', 'error'); return; }

    const variants = window._abVariants.filter(v => v.target);
    if (variants.length < 2) { window.showToast('At least 2 variants with targets required', 'error'); return; }

    document.getElementById('ab-start-btn').disabled = true;

    const res = await api.startAB({
        workload_id: workloadId,
        variants,
        read_only: document.getElementById('ab-readonly').checked,
        speed: parseFloat(document.getElementById('ab-speed').value) || 1.0,
        threshold: parseFloat(document.getElementById('ab-threshold').value) || 20.0,
    });

    if (res.error) {
        window.showToast(res.error, 'error');
        document.getElementById('ab-start-btn').disabled = false;
    } else {
        window.showToast('A/B test started', 'success');
    }
}

function renderABResults(report) {
    const el = document.getElementById('ab-results');
    if (!el) return;
    el.classList.remove('hidden');

    const winner = report.variants.reduce((a, b) => a.avg_latency_us < b.avg_latency_us ? a : b);

    el.innerHTML = `
        <div class="card">
            <div class="section-header">
                <h3 class="section-title">Results</h3>
                <span class="badge badge-success">Winner: ${winner.label}</span>
            </div>

            <div class="grid grid-cols-${report.variants.length} gap-4 mb-4">
                ${report.variants.map(v => `
                    <div class="card ${v.label === winner.label ? 'border-accent/40' : ''}">
                        <div class="text-sm font-medium mb-2">${v.label}
                            ${v.label === winner.label ? ' 🏆' : ''}
                        </div>
                        <div class="space-y-1 text-xs font-mono">
                            <div class="flex justify-between"><span class="text-slate-500">avg</span><span>${Tables.formatDuration(v.avg_latency_us)}</span></div>
                            <div class="flex justify-between"><span class="text-slate-500">p50</span><span>${Tables.formatDuration(v.p50_latency_us)}</span></div>
                            <div class="flex justify-between"><span class="text-slate-500">p95</span><span>${Tables.formatDuration(v.p95_latency_us)}</span></div>
                            <div class="flex justify-between"><span class="text-slate-500">p99</span><span>${Tables.formatDuration(v.p99_latency_us)}</span></div>
                            <div class="flex justify-between"><span class="text-slate-500">errors</span><span>${v.total_errors}</span></div>
                        </div>
                    </div>
                `).join('')}
            </div>

            <div class="chart-container tall">
                <canvas id="ab-comparison-chart"></canvas>
            </div>
        </div>

        ${report.regressions && report.regressions.length > 0 ? `
        <div class="card">
            <h3 class="section-title mb-3">Regressions</h3>
            <div class="overflow-x-auto">
                ${Tables.renderTable('ab-regressions', [
                    { label: 'Query', render: r => Tables.truncateSQL(r.sql, 60) },
                    { label: 'Baseline', render: r => Tables.formatDuration(r.baseline_us), align: 'right' },
                    { label: 'Variant', key: 'variant_label' },
                    { label: 'Variant Time', render: r => Tables.formatDuration(r.variant_us), align: 'right' },
                    { label: 'Change', render: r => `<span class="text-danger">+${r.change_pct.toFixed(1)}%</span>`, align: 'right' },
                ], report.regressions)}
            </div>
        </div>
        ` : ''}
    `;

    setTimeout(() => Charts.createComparisonBar('ab-comparison-chart', report.variants), 100);
}
