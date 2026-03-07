// Dashboard page
function dashboardPage() {
    return {
        stats: null,
        recentRuns: [],
        health: null,
        loading: true,

        async load() {
            this.loading = true;
            const el = document.getElementById('dashboard-content');
            if (!el) return;
            el.innerHTML = Status.loading('Loading dashboard...');

            const [healthRes, statsRes, runsRes] = await Promise.all([
                api.health(),
                api.runStats(),
                api.listRuns({ limit: 10 }),
            ]);

            this.health = healthRes;
            this.stats = statsRes.stats || { total: 0, passed: 0, failed: 0, running: 0 };
            this.recentRuns = (runsRes.runs || []).slice(0, 10);
            this.loading = false;
            this.render(el);
        },

        render(el) {
            const s = this.stats;
            el.innerHTML = `
                <div class="fade-in space-y-6">
                    <!-- Status cards -->
                    <div class="grid-stats">
                        ${Status.statCard({ label: 'Total Runs', value: s.total, color: 'accent', icon: '⟳' })}
                        ${Status.statCard({ label: 'Passed', value: s.passed, color: 'accent', icon: '✓' })}
                        ${Status.statCard({ label: 'Failed', value: s.failed, color: 'danger', icon: '✗' })}
                        ${Status.statCard({ label: 'Running', value: s.running, color: 'blue', icon: '▶' })}
                    </div>

                    <!-- Quick actions -->
                    <div class="card">
                        <div class="section-header">
                            <h3 class="section-title">Quick Actions</h3>
                        </div>
                        <div class="flex flex-wrap gap-2">
                            <button class="btn btn-primary" onclick="location.hash='workloads'">
                                Upload Workload
                            </button>
                            <button class="btn btn-secondary" onclick="location.hash='proxy'">
                                Start Proxy
                            </button>
                            <button class="btn btn-secondary" onclick="location.hash='replay'">
                                Run Replay
                            </button>
                            <button class="btn btn-secondary" onclick="location.hash='ab'">
                                A/B Test
                            </button>
                            <button class="btn btn-secondary" onclick="location.hash='pipeline'">
                                Run Pipeline
                            </button>
                        </div>
                    </div>

                    <!-- Recent runs -->
                    <div class="card">
                        <div class="section-header">
                            <h3 class="section-title">Recent Runs</h3>
                            <button class="btn btn-secondary btn-sm" onclick="location.hash='history'">View All</button>
                        </div>
                        <div class="overflow-x-auto">
                            ${this.renderRunsTable()}
                        </div>
                    </div>
                </div>
            `;
        },

        renderRunsTable() {
            if (this.recentRuns.length === 0) {
                return Status.empty('No runs yet. Start a replay or pipeline to see results here.');
            }

            const columns = [
                { label: 'Type', key: 'run_type' },
                { label: 'Status', render: r => Tables.statusBadge(r.status) },
                { label: 'Target', render: r => r.target_conn ? Tables.truncateSQL(r.target_conn, 40) : '—' },
                { label: 'Exit', render: r => Tables.exitCodeBadge(r.exit_code) },
                { label: 'Started', render: r => Tables.formatTimestamp(r.started_at) },
            ];
            return Tables.renderTable('recent-runs', columns, this.recentRuns);
        },
    };
}
