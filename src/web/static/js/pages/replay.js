// Replay page
function replayPage() {
    return {
        workloads: [],
        loading: true,
        activeReplay: null,
        progress: 0,

        async load() {
            const el = document.getElementById('replay-content');
            if (!el) return;
            el.innerHTML = Status.loading();

            const res = await api.listWorkloads();
            this.workloads = res.workloads || [];
            this.loading = false;
            this.render(el);
            this.setupWsListeners();
        },

        setupWsListeners() {
            wsClient.on('ReplayProgress', (msg) => {
                this.progress = msg.pct;
                const bar = document.getElementById('replay-progress');
                if (bar) bar.innerHTML = Status.progressBar(msg.pct, `${msg.completed}/${msg.total} sessions`);
            });
            wsClient.on('ReplayCompleted', (msg) => {
                this.activeReplay = null;
                window.showToast('Replay completed!', 'success');
                this.updateButtons(false);
                const bar = document.getElementById('replay-progress');
                if (bar) bar.innerHTML = Status.progressBar(100, 'Complete');
            });
            wsClient.on('ReplayFailed', (msg) => {
                this.activeReplay = null;
                window.showToast(`Replay failed: ${msg.error}`, 'error');
                this.updateButtons(false);
            });
        },

        updateButtons(running) {
            const startBtn = document.getElementById('replay-start-btn');
            const cancelBtn = document.getElementById('replay-cancel-btn');
            if (startBtn) startBtn.disabled = running;
            if (cancelBtn) cancelBtn.disabled = !running;
        },

        render(el) {
            const wklOptions = this.workloads.map(w =>
                `<option value="${w.id}">${w.name} (${w.total_sessions}s / ${w.total_queries}q)</option>`
            ).join('');

            el.innerHTML = `
            <div class="fade-in space-y-4">
                <div class="grid grid-cols-1 lg:grid-cols-3 gap-4">
                    <!-- Config -->
                    <div class="lg:col-span-2 card">
                        <h3 class="section-title mb-4">Replay Configuration</h3>
                        <div class="space-y-4">
                            <div>
                                <label class="label">Workload</label>
                                <select class="input" id="replay-workload">
                                    <option value="">Select workload...</option>
                                    ${wklOptions}
                                </select>
                            </div>
                            <div>
                                <label class="label">Target Connection String</label>
                                <input class="input" type="text" id="replay-target"
                                       placeholder="postgres://user:pass@host:5432/dbname">
                            </div>
                            <div class="grid grid-cols-3 gap-3">
                                <div>
                                    <label class="label">Speed</label>
                                    <input class="input" type="number" id="replay-speed" value="1.0"
                                           step="0.1" min="0">
                                </div>
                                <div>
                                    <label class="label">Scale</label>
                                    <input class="input" type="number" id="replay-scale" value="1" min="1">
                                </div>
                                <div>
                                    <label class="label">Stagger (ms)</label>
                                    <input class="input" type="number" id="replay-stagger" value="0" min="0">
                                </div>
                            </div>
                            <label class="flex items-center gap-2 cursor-pointer text-sm text-slate-300">
                                <input type="checkbox" id="replay-readonly"
                                       class="w-4 h-4 rounded border-slate-600 bg-slate-800">
                                Read-only mode (strip DML)
                            </label>
                        </div>
                    </div>

                    <!-- Per-category scaling -->
                    <div class="card">
                        <h3 class="section-title mb-4">Per-Category Scale</h3>
                        <p class="text-xs text-slate-500 mb-3">Leave blank for uniform scaling</p>
                        <div class="space-y-3">
                            <div>
                                <label class="label">Analytical</label>
                                <input class="input" type="number" id="replay-scale-analytical" placeholder="—" min="1">
                            </div>
                            <div>
                                <label class="label">Transactional</label>
                                <input class="input" type="number" id="replay-scale-transactional" placeholder="—" min="1">
                            </div>
                            <div>
                                <label class="label">Mixed</label>
                                <input class="input" type="number" id="replay-scale-mixed" placeholder="—" min="1">
                            </div>
                            <div>
                                <label class="label">Bulk</label>
                                <input class="input" type="number" id="replay-scale-bulk" placeholder="—" min="1">
                            </div>
                        </div>
                    </div>
                </div>

                <!-- Controls + Progress -->
                <div class="card">
                    <div class="flex items-center gap-3 mb-3">
                        <button class="btn btn-primary" id="replay-start-btn" onclick="startReplay()">
                            Start Replay
                        </button>
                        <button class="btn btn-danger" id="replay-cancel-btn" onclick="cancelReplay()" disabled>
                            Cancel
                        </button>
                    </div>
                    <div id="replay-progress"></div>
                </div>
            </div>
            `;
        },
    };
}

async function startReplay() {
    const workloadId = document.getElementById('replay-workload').value;
    const target = document.getElementById('replay-target').value;
    if (!workloadId) { window.showToast('Select a workload', 'error'); return; }
    if (!target) { window.showToast('Enter a target connection string', 'error'); return; }

    const scaleA = document.getElementById('replay-scale-analytical').value;
    const scaleT = document.getElementById('replay-scale-transactional').value;
    const scaleM = document.getElementById('replay-scale-mixed').value;
    const scaleB = document.getElementById('replay-scale-bulk').value;

    const config = {
        workload_id: workloadId,
        target,
        read_only: document.getElementById('replay-readonly').checked,
        speed: parseFloat(document.getElementById('replay-speed').value) || 1.0,
        scale: parseInt(document.getElementById('replay-scale').value) || 1,
        stagger_ms: parseInt(document.getElementById('replay-stagger').value) || 0,
    };
    if (scaleA) config.scale_analytical = parseInt(scaleA);
    if (scaleT) config.scale_transactional = parseInt(scaleT);
    if (scaleM) config.scale_mixed = parseInt(scaleM);
    if (scaleB) config.scale_bulk = parseInt(scaleB);

    document.getElementById('replay-start-btn').disabled = true;
    document.getElementById('replay-cancel-btn').disabled = false;

    const res = await api.startReplay(config);
    if (res.error) {
        window.showToast(res.error, 'error');
        document.getElementById('replay-start-btn').disabled = false;
        document.getElementById('replay-cancel-btn').disabled = true;
    } else {
        window.showToast('Replay started', 'success');
        window._activeReplayTaskId = res.task_id;
    }
}

async function cancelReplay() {
    if (window._activeReplayTaskId) {
        await api.cancelReplay(window._activeReplayTaskId);
        window.showToast('Replay cancelled', 'warning');
        document.getElementById('replay-start-btn').disabled = false;
        document.getElementById('replay-cancel-btn').disabled = true;
    }
}
