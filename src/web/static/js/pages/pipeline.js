// Pipeline page
function pipelinePage() {
    return {
        activeRun: null,
        stage: null,

        async load() {
            const el = document.getElementById('pipeline-content');
            if (!el) return;
            this.render(el);
            this.setupWsListeners();
        },

        setupWsListeners() {
            wsClient.on('PipelineStageChanged', (msg) => {
                this.stage = msg.stage;
                const stageEl = document.getElementById('pipeline-stage');
                if (stageEl) stageEl.innerHTML = `<span class="badge badge-info">${msg.stage}</span>`;
            });
            wsClient.on('PipelineCompleted', (msg) => {
                window.showToast(`Pipeline completed (exit ${msg.exit_code})`, msg.exit_code === 0 ? 'success' : 'error');
                document.getElementById('pipeline-run-btn').disabled = false;
            });
        },

        render(el) {
            el.innerHTML = `
            <div class="fade-in space-y-4">
                <!-- TOML import -->
                <div class="card">
                    <div class="section-header">
                        <h3 class="section-title">Pipeline Configuration</h3>
                        <div class="flex gap-2">
                            <button class="btn btn-secondary btn-sm" onclick="validatePipeline()">Validate</button>
                            <button class="btn btn-primary btn-sm" id="pipeline-run-btn" onclick="runPipeline()">Run Pipeline</button>
                        </div>
                    </div>
                    <div>
                        <label class="label">TOML Configuration</label>
                        <textarea class="input" id="pipeline-toml" rows="20" placeholder='# pg-retest pipeline config
[capture]
workload = "workload.wkl"

[replay]
target = "postgres://user:pass@host:5432/dbname"
speed = 1.0
read_only = false

[thresholds]
p95_max_ms = 100.0
regression_threshold_pct = 20.0

[output]
json = "report.json"
junit = "results.xml"'></textarea>
                    </div>
                </div>

                <!-- Validation / Run status -->
                <div id="pipeline-status" class="hidden"></div>

                <!-- Stage progress -->
                <div id="pipeline-stage" class="hidden"></div>
            </div>
            `;
        },
    };
}

async function validatePipeline() {
    const toml = document.getElementById('pipeline-toml').value;
    if (!toml.trim()) { window.showToast('Enter TOML config', 'error'); return; }

    const statusEl = document.getElementById('pipeline-status');
    statusEl.classList.remove('hidden');
    statusEl.innerHTML = Status.loading('Validating...');

    const res = await api.validatePipeline({ config_toml: toml });
    if (res.valid) {
        statusEl.innerHTML = `
            <div class="card border-accent/30">
                <div class="flex items-center gap-2 text-accent text-sm">
                    <span>✓</span> Configuration is valid
                    <span class="text-slate-500 ml-2">
                        capture: ${res.config.has_capture ? 'yes' : 'no'},
                        provision: ${res.config.has_provision ? 'yes' : 'no'},
                        thresholds: ${res.config.has_thresholds ? 'yes' : 'no'},
                        variants: ${res.config.variants}
                    </span>
                </div>
            </div>
        `;
        window.showToast('Config is valid', 'success');
    } else {
        statusEl.innerHTML = Status.error(res.error || 'Invalid configuration');
    }
}

async function runPipeline() {
    const toml = document.getElementById('pipeline-toml').value;
    if (!toml.trim()) { window.showToast('Enter TOML config', 'error'); return; }

    document.getElementById('pipeline-run-btn').disabled = true;
    const stageEl = document.getElementById('pipeline-stage');
    stageEl.classList.remove('hidden');
    stageEl.innerHTML = '<span class="badge badge-info">Starting...</span>';

    const res = await api.startPipeline({ config_toml: toml });
    if (res.error) {
        window.showToast(res.error, 'error');
        document.getElementById('pipeline-run-btn').disabled = false;
    } else {
        window.showToast('Pipeline started', 'success');
    }
}
