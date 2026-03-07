// WebSocket client with auto-reconnect
class WsClient {
    constructor() {
        this.ws = null;
        this.handlers = {};
        this.connected = false;
        this.reconnectDelay = 1000;
        this.maxReconnectDelay = 30000;
        this.currentDelay = this.reconnectDelay;
    }

    connect() {
        const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
        const url = `${proto}//${location.host}/api/v1/ws`;
        this.ws = new WebSocket(url);

        this.ws.onopen = () => {
            this.connected = true;
            this.currentDelay = this.reconnectDelay;
            this.emit('_connected', true);
        };

        this.ws.onclose = () => {
            this.connected = false;
            this.emit('_connected', false);
            setTimeout(() => this.reconnect(), this.currentDelay);
        };

        this.ws.onerror = () => {
            this.ws.close();
        };

        this.ws.onmessage = (event) => {
            try {
                const msg = JSON.parse(event.data);
                if (msg.type) {
                    this.emit(msg.type, msg);
                }
                this.emit('_any', msg);
            } catch (e) {
                console.warn('WS parse error:', e);
            }
        };
    }

    reconnect() {
        this.currentDelay = Math.min(this.currentDelay * 1.5, this.maxReconnectDelay);
        this.connect();
    }

    on(type, handler) {
        if (!this.handlers[type]) this.handlers[type] = [];
        this.handlers[type].push(handler);
        return () => {
            this.handlers[type] = this.handlers[type].filter(h => h !== handler);
        };
    }

    emit(type, data) {
        (this.handlers[type] || []).forEach(h => h(data));
    }

    disconnect() {
        if (this.ws) {
            this.ws.close();
            this.ws = null;
        }
    }
}

const wsClient = new WsClient();
