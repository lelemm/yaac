const { readFileSync } = require("node:fs");
const { join } = require("node:path");
const { setImmediate } = require("node:timers/promises");
const vm = require("node:vm");
const test = require("node:test");
const assert = require("node:assert/strict");

const page = readFileSync(join(__dirname, "../src/voice_call.html"), "utf8");
const script = page.split("<script>")[1].split("</script>")[0];

function load(getMedia) {
    const documentEvents = new EventTarget();
    const outbound = [];
    const audio = { srcObject: null, async play() { this.playing = true; } };
    const channel = new EventTarget();
    let peer;
    class Peer extends EventTarget {
        constructor({ iceServers }) {
            super();
            assert.equal(iceServers.length, 0);
            this.iceGatheringState = "gathering";
            peer = this;
        }
        createDataChannel(label) {
            assert.equal(label, "oai-events");
            return channel;
        }
        addTrack(track, stream) { this.track = track; this.stream = stream; }
        async createOffer() { return { sdp: "incomplete" }; }
        async setLocalDescription(offer) { this.localDescription = offer; }
        async setRemoteDescription(answer) { this.answer = answer; }
        close() { this.closed = true; }
    }
    const mic = Object.assign(new EventTarget(), { enabled: true, stop() { this.stopped = true; } });
    const stream = { getAudioTracks: () => [mic], getTracks: () => [mic] };
    const window = {};
    vm.runInNewContext(script, {
        RTCPeerConnection: Peer,
        MediaStream: class { constructor(tracks) { this.tracks = tracks; } },
        window, console: {
            log(message) { outbound.push(JSON.parse(message.substring("yaac-call:".length))); },
        },
        document: { querySelector: () => audio },
        navigator: { mediaDevices: { getUserMedia: options => getMedia(options, stream) } },
        addEventListener: documentEvents.addEventListener.bind(documentEvents),
        setTimeout, clearTimeout,
    });
    return { window, peer, mic, stream, channel, outbound, documentEvents, audio };
}

test("waits for ICE and uses the data channel to mark the call ready", async () => {
    const call = load(async (options, stream) => {
        assert.equal(options.video, false);
        assert.equal(options.audio.echoCancellation, true);
        assert.equal(options.audio.noiseSuppression, true);
        assert.equal(options.audio.autoGainControl, true);
        return stream;
    });
    await setImmediate();
    assert.deepEqual(call.outbound, []);
    assert.equal(call.peer.track, call.mic);
    assert.equal(call.peer.stream, call.stream);
    call.peer.iceGatheringState = "complete";
    call.peer.localDescription.sdp = "complete";
    call.peer.dispatchEvent(new Event("icegatheringstatechange"));
    await setImmediate();
    assert.deepEqual(call.outbound, [{ Offer: "complete" }]);
    await call.window.yaacCall.answer("answer");
    assert.equal(call.peer.answer.type, "answer");
    assert.equal(call.peer.answer.sdp, "answer");
    call.channel.dispatchEvent(new Event("open"));
    assert.deepEqual(call.outbound, [{ Offer: "complete" }, "Ready"]);
    const remote = new EventTarget();
    call.peer.dispatchEvent(Object.assign(new Event("track"), { track: remote }));
    assert.equal(call.audio.srcObject.tracks[0], remote);
    assert.equal(call.audio.playing, true);
    call.window.yaacCall.mute(true);
    assert.equal(call.mic.enabled, false);
    call.window.yaacCall.mute(false);
    assert.equal(call.mic.enabled, true);
    call.documentEvents.dispatchEvent(new Event("pagehide"));
    assert.equal(call.peer.closed, true);
    assert.equal(call.mic.stopped, true);
    assert.equal(call.audio.srcObject, null);
});

test("releases a microphone that arrives after the call closes", async () => {
    let allowCapture;
    const call = load((_, stream) => new Promise(resolve => {
        allowCapture = () => resolve(stream);
    }));
    call.documentEvents.dispatchEvent(new Event("pagehide"));
    allowCapture();
    await setImmediate();
    assert.equal(call.peer.closed, true);
    assert.equal(call.mic.stopped, true);
    assert.deepEqual(call.outbound, []);
});
