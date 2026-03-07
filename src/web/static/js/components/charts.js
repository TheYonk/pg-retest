// Chart.js helpers for pg-retest dashboard

const chartColors = {
    accent: 'rgba(45, 212, 191, 1)',
    accentDim: 'rgba(45, 212, 191, 0.3)',
    accentBg: 'rgba(45, 212, 191, 0.1)',
    amber: 'rgba(251, 191, 36, 1)',
    amberDim: 'rgba(251, 191, 36, 0.3)',
    danger: 'rgba(244, 63, 94, 1)',
    dangerDim: 'rgba(244, 63, 94, 0.3)',
    blue: 'rgba(96, 165, 250, 1)',
    blueDim: 'rgba(96, 165, 250, 0.3)',
    purple: 'rgba(168, 85, 247, 1)',
    purpleDim: 'rgba(168, 85, 247, 0.3)',
    slate: 'rgba(148, 163, 184, 0.5)',
    grid: 'rgba(51, 65, 85, 0.3)',
};

const variantColors = [
    { bg: chartColors.accentDim, border: chartColors.accent },
    { bg: chartColors.amberDim, border: chartColors.amber },
    { bg: chartColors.dangerDim, border: chartColors.danger },
    { bg: chartColors.blueDim, border: chartColors.blue },
    { bg: chartColors.purpleDim, border: chartColors.purple },
];

const defaultChartOptions = {
    responsive: true,
    maintainAspectRatio: false,
    plugins: {
        legend: {
            labels: {
                color: '#94a3b8',
                font: { family: 'JetBrains Mono', size: 11 },
                padding: 16,
                usePointStyle: true,
                pointStyleWidth: 8,
            },
        },
        tooltip: {
            backgroundColor: 'rgba(15, 23, 42, 0.95)',
            titleColor: '#e2e8f0',
            bodyColor: '#94a3b8',
            borderColor: 'rgba(51, 65, 85, 0.5)',
            borderWidth: 1,
            padding: 10,
            titleFont: { family: 'DM Sans', size: 12, weight: 600 },
            bodyFont: { family: 'JetBrains Mono', size: 11 },
            cornerRadius: 8,
        },
    },
    scales: {
        x: {
            grid: { color: chartColors.grid },
            ticks: { color: '#64748b', font: { family: 'JetBrains Mono', size: 10 } },
        },
        y: {
            grid: { color: chartColors.grid },
            ticks: { color: '#64748b', font: { family: 'JetBrains Mono', size: 10 } },
        },
    },
};

const Charts = {
    _instances: {},

    destroy(canvasId) {
        if (this._instances[canvasId]) {
            this._instances[canvasId].destroy();
            delete this._instances[canvasId];
        }
    },

    createLatencyChart(canvasId, report) {
        this.destroy(canvasId);
        const ctx = document.getElementById(canvasId);
        if (!ctx) return;

        this._instances[canvasId] = new Chart(ctx, {
            type: 'bar',
            data: {
                labels: ['p50', 'p95', 'p99', 'avg'],
                datasets: [
                    {
                        label: 'Source',
                        data: [
                            report.source_p50_latency_us / 1000,
                            report.source_p95_latency_us / 1000,
                            report.source_p99_latency_us / 1000,
                            report.source_avg_latency_us / 1000,
                        ],
                        backgroundColor: chartColors.accentDim,
                        borderColor: chartColors.accent,
                        borderWidth: 1,
                        borderRadius: 4,
                    },
                    {
                        label: 'Replay',
                        data: [
                            report.replay_p50_latency_us / 1000,
                            report.replay_p95_latency_us / 1000,
                            report.replay_p99_latency_us / 1000,
                            report.replay_avg_latency_us / 1000,
                        ],
                        backgroundColor: chartColors.amberDim,
                        borderColor: chartColors.amber,
                        borderWidth: 1,
                        borderRadius: 4,
                    },
                ],
            },
            options: {
                ...defaultChartOptions,
                plugins: {
                    ...defaultChartOptions.plugins,
                    title: {
                        display: true,
                        text: 'Latency Comparison (ms)',
                        color: '#e2e8f0',
                        font: { family: 'DM Sans', size: 13, weight: 600 },
                    },
                },
            },
        });
    },

    createComparisonBar(canvasId, variants) {
        this.destroy(canvasId);
        const ctx = document.getElementById(canvasId);
        if (!ctx) return;

        const labels = ['p50', 'p95', 'p99', 'avg'];
        const datasets = variants.map((v, i) => ({
            label: v.label,
            data: [
                v.p50_latency_us / 1000,
                v.p95_latency_us / 1000,
                v.p99_latency_us / 1000,
                v.avg_latency_us / 1000,
            ],
            backgroundColor: variantColors[i % variantColors.length].bg,
            borderColor: variantColors[i % variantColors.length].border,
            borderWidth: 1,
            borderRadius: 4,
        }));

        this._instances[canvasId] = new Chart(ctx, {
            type: 'bar',
            data: { labels, datasets },
            options: {
                ...defaultChartOptions,
                plugins: {
                    ...defaultChartOptions.plugins,
                    title: {
                        display: true,
                        text: 'Variant Latency Comparison (ms)',
                        color: '#e2e8f0',
                        font: { family: 'DM Sans', size: 13, weight: 600 },
                    },
                },
            },
        });
    },

    createTrendChart(canvasId, trendData) {
        this.destroy(canvasId);
        const ctx = document.getElementById(canvasId);
        if (!ctx) return;

        const labels = trendData.map((d, i) => d.started_at ? new Date(d.started_at).toLocaleDateString() : `#${i + 1}`);
        const p95Values = trendData.map(d => {
            if (d.report && d.report.replay_p95_latency_us) {
                return d.report.replay_p95_latency_us / 1000;
            }
            return null;
        }).reverse();

        this._instances[canvasId] = new Chart(ctx, {
            type: 'line',
            data: {
                labels: labels.reverse(),
                datasets: [{
                    label: 'p95 Latency (ms)',
                    data: p95Values,
                    borderColor: chartColors.accent,
                    backgroundColor: chartColors.accentBg,
                    fill: true,
                    tension: 0.3,
                    pointRadius: 4,
                    pointBackgroundColor: chartColors.accent,
                    pointBorderColor: 'rgba(15, 23, 42, 1)',
                    pointBorderWidth: 2,
                }],
            },
            options: {
                ...defaultChartOptions,
                plugins: {
                    ...defaultChartOptions.plugins,
                    title: {
                        display: true,
                        text: 'p95 Latency Trend (ms)',
                        color: '#e2e8f0',
                        font: { family: 'DM Sans', size: 13, weight: 600 },
                    },
                },
            },
        });
    },

    createPieChart(canvasId, data, labels) {
        this.destroy(canvasId);
        const ctx = document.getElementById(canvasId);
        if (!ctx) return;

        const colors = [chartColors.accent, chartColors.amber, chartColors.blue, chartColors.purple, chartColors.danger];
        const bgColors = [chartColors.accentDim, chartColors.amberDim, chartColors.blueDim, chartColors.purpleDim, chartColors.dangerDim];

        this._instances[canvasId] = new Chart(ctx, {
            type: 'doughnut',
            data: {
                labels,
                datasets: [{
                    data,
                    backgroundColor: bgColors.slice(0, data.length),
                    borderColor: colors.slice(0, data.length),
                    borderWidth: 1,
                }],
            },
            options: {
                responsive: true,
                maintainAspectRatio: false,
                plugins: {
                    legend: {
                        position: 'right',
                        labels: {
                            color: '#94a3b8',
                            font: { family: 'JetBrains Mono', size: 11 },
                            padding: 12,
                            usePointStyle: true,
                        },
                    },
                    tooltip: defaultChartOptions.plugins.tooltip,
                },
            },
        });
    },

    createQPSChart(canvasId) {
        this.destroy(canvasId);
        const ctx = document.getElementById(canvasId);
        if (!ctx) return;

        this._instances[canvasId] = new Chart(ctx, {
            type: 'line',
            data: {
                labels: Array(60).fill(''),
                datasets: [{
                    label: 'QPS',
                    data: Array(60).fill(0),
                    borderColor: chartColors.accent,
                    backgroundColor: chartColors.accentBg,
                    fill: true,
                    tension: 0.4,
                    pointRadius: 0,
                }],
            },
            options: {
                ...defaultChartOptions,
                animation: false,
                scales: {
                    ...defaultChartOptions.scales,
                    x: { ...defaultChartOptions.scales.x, display: false },
                    y: { ...defaultChartOptions.scales.y, beginAtZero: true },
                },
                plugins: {
                    ...defaultChartOptions.plugins,
                    legend: { display: false },
                },
            },
        });
        return this._instances[canvasId];
    },

    updateQPSChart(canvasId, qps) {
        const chart = this._instances[canvasId];
        if (!chart) return;
        chart.data.datasets[0].data.push(qps);
        chart.data.datasets[0].data.shift();
        chart.data.labels.push('');
        chart.data.labels.shift();
        chart.update('none');
    },
};
