// API client wrapper for pg-retest web dashboard
const API_BASE = '/api/v1';

const api = {
    async request(method, path, body = null) {
        const opts = {
            method,
            headers: {},
        };
        if (body && !(body instanceof FormData)) {
            opts.headers['Content-Type'] = 'application/json';
            opts.body = JSON.stringify(body);
        } else if (body instanceof FormData) {
            opts.body = body;
        }
        try {
            const res = await fetch(`${API_BASE}${path}`, opts);
            const text = await res.text();
            let data;
            try {
                data = text ? JSON.parse(text) : {};
            } catch {
                data = {};
            }
            if (!res.ok && !data.error) {
                data.error = text || `HTTP ${res.status}`;
            }
            return data;
        } catch (e) {
            return { error: e.message };
        }
    },

    get(path) { return this.request('GET', path); },
    post(path, body) { return this.request('POST', path, body); },
    del(path) { return this.request('DELETE', path); },

    // Health
    health() { return this.get('/health'); },
    tasks() { return this.get('/tasks'); },

    // Workloads
    listWorkloads() { return this.get('/workloads'); },
    getWorkload(id) { return this.get(`/workloads/${id}`); },
    inspectWorkload(id) { return this.get(`/workloads/${id}/inspect`); },
    deleteWorkload(id) { return this.del(`/workloads/${id}`); },
    uploadWorkload(formData) { return this.request('POST', '/workloads/upload', formData); },
    importWorkload(formData) { return this.request('POST', '/workloads/import', formData); },

    // Proxy
    proxyStatus() { return this.get('/proxy/status'); },
    startProxy(config) { return this.post('/proxy/start', config); },
    stopProxy() { return this.post('/proxy/stop'); },
    toggleCapture() { return this.post('/proxy/toggle-capture'); },
    proxySessions() { return this.get('/proxy/sessions'); },

    // Replay
    startReplay(config) { return this.post('/replay/start', config); },
    getReplay(id) { return this.get(`/replay/${id}`); },
    cancelReplay(id) { return this.post(`/replay/${id}/cancel`); },

    // Compare
    computeCompare(config) { return this.post('/compare', config); },
    getCompare(runId) { return this.get(`/compare/${runId}`); },

    // A/B
    startAB(config) { return this.post('/ab/start', config); },
    getAB(id) { return this.get(`/ab/${id}`); },

    // Pipeline
    startPipeline(config) { return this.post('/pipeline/start', config); },
    validatePipeline(config) { return this.post('/pipeline/validate', config); },
    getPipeline(id) { return this.get(`/pipeline/${id}`); },

    // Runs
    listRuns(params = {}) {
        const qs = new URLSearchParams(params).toString();
        return this.get(`/runs${qs ? '?' + qs : ''}`);
    },
    getRun(id) { return this.get(`/runs/${id}`); },
    runStats() { return this.get('/runs/stats'); },
    runTrends(params = {}) {
        const qs = new URLSearchParams(params).toString();
        return this.get(`/runs/trends${qs ? '?' + qs : ''}`);
    },
};
