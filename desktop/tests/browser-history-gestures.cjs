// Run with: node --test desktop/tests/browser-history-gestures.cjs
const { test } = require('node:test');
const assert = require('node:assert/strict');
const { readFileSync } = require('node:fs');
const { runInNewContext } = require('node:vm');
const script = readFileSync(`${__dirname}/../src/browser/history_gestures.js`, 'utf8');

function page() {
    class Element {
        constructor(style = {}, width = 0, left = 0) {
            this.style = { overflowX: 'auto', overscrollBehaviorX: 'auto', direction: 'ltr', ...style };
            this.scrollWidth = 100 + width;
            this.clientWidth = 100;
            this.scrollLeft = left;
        }
    }
    const root = new Element();
    const requests = [];
    const tasks = [];
    let handler, now = 0;
    const context = { Element, document: { documentElement: root },
        getComputedStyle: el => el.style, performance: { now: () => now },
        setTimeout: fn => tasks.push(fn),
        window: { location: { assign: url => requests.push(url) },
            addEventListener: (type, fn) => { assert.equal(type, 'wheel'); handler = fn; } } };
    runInNewContext(script, context);
    return { root, Element, requests,
        wheel(deltaX, options = {}, path = [root]) {
            now += 20;
            const event = { deltaX, deltaY: 0, deltaMode: 0, isTrusted: true,
                composedPath: () => path, ...options };
            handler(event);
            if (options.cancelAfterListener) event.defaultPrevented = true;
            while (tasks.length) tasks.shift()();
        },
        pause() { now += 250; } };
}

test('one navigation per continuous swipe, with both directions and threshold', () => {
    const p = page();
    p.wheel(-80); assert.deepEqual(p.requests, []);
    p.wheel(-80); p.wheel(-300);
    assert.deepEqual(p.requests, ['xd-browser-history://back/']);
    p.pause(); p.wheel(160);
    assert.deepEqual(p.requests, ['xd-browser-history://back/', 'xd-browser-history://forward/']);
});

test('scrolling, zoom, canceled, untrusted and wheel-line events never navigate', () => {
    for (const options of [{ deltaY: 160 }, { ctrlKey: true }, { metaKey: true },
        { shiftKey: true }, { altKey: true }, { defaultPrevented: true },
        { isTrusted: false }, { deltaMode: 1 }, { cancelAfterListener: true }]) {
        const p = page(); p.wheel(-200, options); p.wheel(-200);
        assert.deepEqual(p.requests, [], JSON.stringify(options));
    }
});

test('a swipe that starts in scrollable content remains scrolling at its edge', () => {
    const p = page(); const scroller = new p.Element({}, 300, 150);
    p.wheel(-80, {}, [scroller, p.root]);
    scroller.scrollLeft = 0;
    p.wheel(-200, {}, [scroller, p.root]);
    assert.deepEqual(p.requests, []);
    p.pause(); p.wheel(-160, {}, [scroller, p.root]);
    assert.deepEqual(p.requests, ['xd-browser-history://back/']);
});

test('nested scroll containers, overscroll containment and RTL are respected', () => {
    for (const scroller of [
        { style: { overscrollBehaviorX: 'contain' } },
        { style: { overscrollBehaviorX: 'none' } },
        { style: { direction: 'rtl' }, width: 300, left: 0 },
    ]) {
        const p = page(); const el = new p.Element(scroller.style, scroller.width, scroller.left);
        p.wheel(-200, {}, [el, p.root]);
        assert.deepEqual(p.requests, []);
    }
});

test('direction reversal cancels until the next gesture', () => {
    const p = page(); p.wheel(-80); p.wheel(200); p.wheel(-200);
    assert.deepEqual(p.requests, []);
    p.pause(); p.wheel(-160);
    assert.deepEqual(p.requests, ['xd-browser-history://back/']);
});
