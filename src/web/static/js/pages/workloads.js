// Workloads page
function workloadsPage() {
    return {
        workloads: [],
        loading: true,
        showUpload: false,
        showDetail: false,
        selectedWorkload: null,
        inspectData: null,

        async load() {
            this.loading = true;
            const el = document.getElementById('workloads-content');
            if (!el) return;
            el.innerHTML = Status.loading();

            const res = await api.listWorkloads();
            this.workloads = res.workloads || [];
            this.loading = false;
            this.render(el);
        },

        render(el) {
            el.innerHTML = `
            <div class="fade-in space-y-4">
                <div class="section-header">
                    <h3 class="section-title">Workload Profiles</h3>
                    <div class="flex gap-2">
                        <button class="btn btn-primary" onclick="workloadUploadModal('upload')">
                            Upload Log
                        </button>
                        <button class="btn btn-secondary" onclick="workloadUploadModal('import')">
                            Import .wkl
                        </button>
                    </div>
                </div>

                ${this.workloads.length === 0
                    ? Status.empty('No workloads. Upload a log file or import a .wkl profile.')
                    : `<div class="card overflow-x-auto">${this.renderTable()}</div>`
                }
            </div>

            <!-- Upload modal -->
            <div id="upload-modal" class="hidden">
                <div class="modal-overlay" onclick="closeUploadModal()">
                    <div class="modal-content" onclick="event.stopPropagation()">
                        <h3 class="text-base font-semibold mb-4" id="upload-modal-title">Upload</h3>
                        <form id="upload-form" onsubmit="handleUpload(event)">
                            <div class="space-y-4">
                                <div>
                                    <label class="label">File</label>
                                    <input type="file" name="file" required
                                           class="input text-sm file:mr-4 file:py-1 file:px-3 file:rounded-md file:border-0 file:bg-accent/20 file:text-accent file:text-xs file:cursor-pointer">
                                </div>
                                <div id="upload-extra-fields"></div>
                                <div class="flex justify-end gap-2 pt-2">
                                    <button type="button" class="btn btn-secondary" onclick="closeUploadModal()">Cancel</button>
                                    <button type="submit" class="btn btn-primary">Upload</button>
                                </div>
                            </div>
                        </form>
                        <div id="upload-status" class="mt-3 hidden"></div>
                    </div>
                </div>
            </div>

            <!-- Detail modal -->
            <div id="detail-modal" class="hidden">
                <div class="modal-overlay" onclick="closeDetailModal()">
                    <div class="modal-content" style="max-width: 800px" onclick="event.stopPropagation()">
                        <div id="detail-modal-content"></div>
                    </div>
                </div>
            </div>
            `;
        },

        renderTable() {
            const columns = [
                { label: 'Name', key: 'name' },
                { label: 'Source', key: 'source_type' },
                { label: 'Host', key: 'source_host' },
                { label: 'Sessions', key: 'total_sessions', align: 'right' },
                { label: 'Queries', key: 'total_queries', align: 'right' },
                { label: 'Captured', render: r => Tables.formatTimestamp(r.captured_at) },
                { label: '', render: r => `
                    <div class="flex gap-1">
                        <button class="btn btn-secondary btn-sm" onclick="inspectWorkload('${r.id}')">Inspect</button>
                        <button class="btn btn-danger btn-sm" onclick="deleteWorkload('${r.id}')">Delete</button>
                    </div>
                `},
            ];
            return Tables.renderTable('workloads-table', columns, this.workloads);
        },
    };
}

// Global handlers for workload actions
function workloadUploadModal(mode) {
    const modal = document.getElementById('upload-modal');
    const title = document.getElementById('upload-modal-title');
    const extra = document.getElementById('upload-extra-fields');
    const form = document.getElementById('upload-form');

    form.dataset.mode = mode;
    if (mode === 'upload') {
        title.textContent = 'Upload Log File';
        extra.innerHTML = `
            <div>
                <label class="label">Source Type</label>
                <select class="input" name="source_type">
                    <option value="pg-csv">PostgreSQL CSV Log</option>
                    <option value="mysql-slow">MySQL Slow Log</option>
                </select>
            </div>
            <div>
                <label class="label">Source Host</label>
                <input class="input" name="source_host" placeholder="production-db-01" value="uploaded">
            </div>
            <label class="flex items-center gap-2 cursor-pointer text-sm text-slate-300">
                <input type="checkbox" name="mask_values" class="w-4 h-4 rounded border-slate-600 bg-slate-800">
                Mask PII (strings/numbers)
            </label>
        `;
    } else {
        title.textContent = 'Import .wkl Profile';
        extra.innerHTML = '';
    }

    modal.classList.remove('hidden');
}

function closeUploadModal() {
    document.getElementById('upload-modal').classList.add('hidden');
}

async function handleUpload(e) {
    e.preventDefault();
    const form = e.target;
    const mode = form.dataset.mode;
    const statusEl = document.getElementById('upload-status');
    statusEl.classList.remove('hidden');
    statusEl.innerHTML = Status.loading('Uploading...');

    const formData = new FormData(form);
    let res;
    if (mode === 'import') {
        res = await api.importWorkload(formData);
    } else {
        res = await api.uploadWorkload(formData);
    }

    if (res.error) {
        statusEl.innerHTML = Status.error(res.error);
    } else {
        closeUploadModal();
        window.showToast('Workload uploaded successfully', 'success');
        // Reload workloads list
        const page = Alpine.$data(document.querySelector('[x-data="workloadsPage()"]'));
        if (page) page.load();
    }
}

async function inspectWorkload(id) {
    const modal = document.getElementById('detail-modal');
    const content = document.getElementById('detail-modal-content');
    content.innerHTML = Status.loading('Loading workload details...');
    modal.classList.remove('hidden');

    const res = await api.inspectWorkload(id);
    if (res.error) {
        content.innerHTML = Status.error(res.error);
        return;
    }

    const profile = res.profile;
    const classification = res.classification;

    content.innerHTML = `
        <div class="space-y-4">
            <div class="flex items-center justify-between">
                <h3 class="text-base font-semibold">Workload Details</h3>
                <button class="btn btn-secondary btn-sm" onclick="closeDetailModal()">Close</button>
            </div>

            <div class="grid grid-cols-2 gap-4">
                <div class="card">
                    <div class="text-xs text-slate-500 uppercase mb-1">Source</div>
                    <div class="font-mono text-sm">${profile.source_host}</div>
                </div>
                <div class="card">
                    <div class="text-xs text-slate-500 uppercase mb-1">Capture Method</div>
                    <div class="font-mono text-sm">${profile.capture_method}</div>
                </div>
                <div class="card">
                    <div class="text-xs text-slate-500 uppercase mb-1">Sessions</div>
                    <div class="font-mono text-sm text-accent">${profile.metadata.total_sessions}</div>
                </div>
                <div class="card">
                    <div class="text-xs text-slate-500 uppercase mb-1">Queries</div>
                    <div class="font-mono text-sm text-accent">${profile.metadata.total_queries}</div>
                </div>
            </div>

            ${classification ? `
            <div class="card">
                <h4 class="text-sm font-medium mb-3">Classification</h4>
                <div class="flex items-center gap-4 mb-3">
                    <span class="badge badge-info">${classification.overall_class}</span>
                    <span class="text-xs text-slate-500">
                        A:${classification.class_counts.analytical || 0}
                        T:${classification.class_counts.transactional || 0}
                        M:${classification.class_counts.mixed || 0}
                        B:${classification.class_counts.bulk || 0}
                    </span>
                </div>
                <div class="chart-container" style="height: 200px">
                    <canvas id="classification-chart"></canvas>
                </div>
            </div>
            ` : ''}
        </div>
    `;

    if (classification) {
        const cc = classification.class_counts;
        const data = [cc.analytical || 0, cc.transactional || 0, cc.mixed || 0, cc.bulk || 0];
        const labels = ['Analytical', 'Transactional', 'Mixed', 'Bulk'];
        setTimeout(() => Charts.createPieChart('classification-chart', data, labels), 100);
    }
}

function closeDetailModal() {
    document.getElementById('detail-modal').classList.add('hidden');
}

async function deleteWorkload(id) {
    if (!confirm('Delete this workload?')) return;
    const res = await api.deleteWorkload(id);
    if (res.error) {
        window.showToast(res.error, 'error');
    } else {
        window.showToast('Workload deleted', 'success');
        const page = Alpine.$data(document.querySelector('[x-data="workloadsPage()"]'));
        if (page) page.load();
    }
}
