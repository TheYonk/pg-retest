// Proxy control page
function proxyPage() {
    return {
        status: null,
        queryFeed: [],
        qpsChart: null,
        loading: true,
        config: {
            listen: '0.0.0.0:5433',
            target: '',
            pool_size: 100,
            mask_values: false,
            no_capture: false,
        },

        async load() {
            const el = document.getElementById('proxy-content');
            if (!el) return;
            el.innerHTML = Status.loading();

            const res = await api.proxyStatus();
            this.status = res;
            this.loading = false;
            this.render(el);
            this.setupWsListeners();
        },

        setupWsListeners() {
            wsClient.on('ProxyQueryExecuted', (msg) => {
                this.queryFeed.unshift(msg);
                if (this.queryFeed.length > 100) this.queryFeed.pop();
                this.renderFeed();
            });
            wsClient.on('ProxyStats', (msg) => {
                Charts.updateQPSChart('qps-chart', msg.qps || 0);
            });
            wsClient.on('ProxyStarted', () => {
                this.status = { ...this.status, running: true };
                this.renderStatus();
            });
            wsClient.on('ProxyStopped', () => {
                this.status = { ...this.status, running: false };
                this.renderStatus();
            });
        },

        render(el) {
            el.innerHTML = `
            <div class="fade-in space-y-4">
                <!-- Proxy status & controls -->
                <div class="grid grid-cols-1 lg:grid-cols-3 gap-4">
                    <div class="lg:col-span-2 card">
                        <div class="section-header">
                            <h3 class="section-title">Proxy Configuration</h3>
                            <div id="proxy-status-badge"></div>
                        </div>
                        <div class="space-y-3">
                            <div class="grid grid-cols-2 gap-3">
                                <div>
                                    <label class="label">Listen Address</label>
                                    <input class="input" type="text" id="proxy-listen"
                                           value="${this.config.listen}" placeholder="0.0.0.0:5433">
                                </div>
                                <div>
                                    <label class="label">Target PostgreSQL</label>
                                    <input class="input" type="text" id="proxy-target"
                                           value="${this.config.target}" placeholder="localhost:5432">
                                </div>
                            </div>
                            <div class="grid grid-cols-3 gap-3">
                                <div>
                                    <label class="label">Pool Size</label>
                                    <input class="input" type="number" id="proxy-pool-size"
                                           value="${this.config.pool_size}">
                                </div>
                                <div class="flex items-end pb-1">
                                    <label class="flex items-center gap-2 cursor-pointer text-sm text-slate-300">
                                        <input type="checkbox" id="proxy-mask"
                                               class="w-4 h-4 rounded border-slate-600 bg-slate-800">
                                        Mask PII
                                    </label>
                                </div>
                                <div class="flex items-end pb-1">
                                    <label class="flex items-center gap-2 cursor-pointer text-sm text-slate-300">
                                        <input type="checkbox" id="proxy-no-capture"
                                               class="w-4 h-4 rounded border-slate-600 bg-slate-800">
                                        No Capture
                                    </label>
                                </div>
                            </div>
                            <div class="flex gap-2 pt-2">
                                <button class="btn btn-primary" id="proxy-start-btn" onclick="startProxy()">
                                    Start Proxy
                                </button>
                                <button class="btn btn-danger" id="proxy-stop-btn" onclick="stopProxy()" disabled>
                                    Stop Proxy
                                </button>
                            </div>
                        </div>
                    </div>

                    <!-- QPS chart -->
                    <div class="card">
                        <h3 class="section-title mb-2">Queries/sec</h3>
                        <div class="chart-container" style="height: 180px">
                            <canvas id="qps-chart"></canvas>
                        </div>
                    </div>
                </div>

                <!-- Live query feed -->
                <div class="card">
                    <div class="section-header">
                        <h3 class="section-title">Live Query Feed</h3>
                        <button class="btn btn-secondary btn-sm" onclick="clearQueryFeed()">Clear</button>
                    </div>
                    <div class="query-feed" id="query-feed">
                        <div class="text-center text-slate-500 text-sm py-4">
                            Start the proxy to see live queries
                        </div>
                    </div>
                </div>
            </div>
            `;

            Charts.createQPSChart('qps-chart');
            this.renderStatus();
        },

        renderStatus() {
            const badge = document.getElementById('proxy-status-badge');
            const startBtn = document.getElementById('proxy-start-btn');
            const stopBtn = document.getElementById('proxy-stop-btn');
            if (!badge) return;

            if (this.status && this.status.running) {
                badge.innerHTML = '<span class="badge badge-success">Running</span>';
                if (startBtn) startBtn.disabled = true;
                if (stopBtn) stopBtn.disabled = false;
            } else {
                badge.innerHTML = '<span class="badge badge-neutral">Stopped</span>';
                if (startBtn) startBtn.disabled = false;
                if (stopBtn) stopBtn.disabled = true;
            }
        },

        renderFeed() {
            const feed = document.getElementById('query-feed');
            if (!feed || this.queryFeed.length === 0) return;
            feed.innerHTML = this.queryFeed.map(q => `
                <div class="query-feed-item">
                    <span class="text-slate-500 flex-shrink-0">S${q.session_id}</span>
                    <span class="text-slate-300 flex-1">${Tables.truncateSQL(q.sql_preview, 120)}</span>
                    <span class="text-accent flex-shrink-0">${Tables.formatDuration(q.duration_us)}</span>
                </div>
            `).join('');
        },
    };
}

async function startProxy() {
    const config = {
        listen: document.getElementById('proxy-listen').value,
        target: document.getElementById('proxy-target').value,
        pool_size: parseInt(document.getElementById('proxy-pool-size').value) || 100,
        mask_values: document.getElementById('proxy-mask').checked,
        no_capture: document.getElementById('proxy-no-capture').checked,
    };
    if (!config.target) {
        window.showToast('Target address is required', 'error');
        return;
    }
    const res = await api.startProxy(config);
    if (res.error) {
        window.showToast(res.error, 'error');
    } else {
        window.showToast('Proxy started', 'success');
    }
}

async function stopProxy() {
    const res = await api.stopProxy();
    if (res.error) {
        window.showToast(res.error, 'error');
    } else {
        window.showToast('Proxy stopped', 'success');
    }
}

function clearQueryFeed() {
    const feed = document.getElementById('query-feed');
    if (feed) feed.innerHTML = '<div class="text-center text-slate-500 text-sm py-4">Feed cleared</div>';
}
