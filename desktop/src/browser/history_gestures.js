// CEF Alloy has no native overscroll history controller. Leave scrolling to
// the page; only a gesture that starts at an uncontained horizontal edge may
// request history navigation. The host cancels this private URL and navigates.
(() => {
    if (window.__xdHistoryGestures) return;
    window.__xdHistoryGestures = true;
    let last = -Infinity;
    let distance = 0;
    let direction = 0;
    let blocked = false;

    function pageConsumes(event, delta) {
        const path = event.composedPath();
        if (!path.includes(document.documentElement)) path.push(document.documentElement);
        return path.some(element => {
            if (!(element instanceof Element)) return false;
            const style = getComputedStyle(element);
            const root = element === document.documentElement;
            if (!root && !/^(auto|scroll)$/.test(style.overflowX)) return false;
            if (style.overscrollBehaviorX === 'contain' || style.overscrollBehaviorX === 'none') return true;
            const width = element.scrollWidth - element.clientWidth;
            if (width <= 1) return false;
            // Chromium exposes negative scrollLeft for right-to-left content.
            const left = style.direction === 'rtl' ? element.scrollLeft + width : element.scrollLeft;
            return delta < 0 ? left > 1 : left < width - 1;
        });
    }

    window.addEventListener('wheel', event => {
        const now = performance.now();
        if (now - last > 200) {
            distance = 0;
            direction = 0;
            blocked = false;
        }
        last = now;
        // A later page listener may cancel this event after ours runs.
        setTimeout(() => { if (event.defaultPrevented) blocked = true; }, 0);
        // Pixel deltas are continuous touchpad scrolling. Line/page wheels,
        // zoom, vertical/diagonal scrolling and canceled events stay untouched.
        if (!event.isTrusted || event.defaultPrevented || event.deltaMode !== 0 ||
            event.ctrlKey || event.altKey || event.metaKey || event.shiftKey ||
            Math.abs(event.deltaX) <= Math.abs(event.deltaY) * 1.5) {
            blocked = true;
        }
        if (blocked || event.deltaX === 0) return;
        const sign = Math.sign(event.deltaX);
        if (!direction) {
            direction = sign;
            blocked = pageConsumes(event, event.deltaX);
        }
        if (blocked) return;
        if (sign !== direction) {
            // Reversing a swipe cancels it until the next gesture.
            blocked = true;
            return;
        }
        distance += Math.abs(event.deltaX);
        if (distance >= 160) {
            blocked = true;
            const url = direction < 0 ?
                'xd-browser-history://back/' : 'xd-browser-history://forward/';
            setTimeout(() => {
                if (!event.defaultPrevented) window.location.assign(url);
            }, 0);
        }
    }, { passive: true });
})();
