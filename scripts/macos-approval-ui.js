// Remote runner only: snapshot accessibility containers and click an unchanged selection.
function clipElement(position, size, viewport, button) {
    const rect = [position[0], position[1], position[0] + size[0], position[1] + size[1]];
    if (!viewport) return {visible: true, clip: rect};
    const clip = [Math.max(rect[0], viewport[0]), Math.max(rect[1], viewport[1]), Math.min(rect[2], viewport[2]), Math.min(rect[3], viewport[3])];
    const center = [position[0] + size[0] / 2, position[1] + size[1] / 2];
    return {clip: clip, visible: clip[0] < clip[2] && clip[1] < clip[3] && (!button ||
        (center[0] >= viewport[0] && center[0] < viewport[2] && center[1] >= viewport[1] && center[1] < viewport[3]))};
}
function optionalBoolean(getter) {
    try { return Boolean(getter()); } catch (_) { return null; }
}
function doublePair(encoded) {
    const alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    const bytes = [];
    let bits = 0, value = 0;
    for (const char of encoded) {
        if (char === "=") break;
        const digit = alphabet.indexOf(char);
        if (digit < 0) throw new Error("Invalid AX geometry bytes");
        value = (value << 6) | digit;
        bits += 6;
        if (bits >= 8) { bits -= 8; bytes.push((value >> bits) & 255); }
    }
    if (bytes.length !== 16) throw new Error("Unexpected AX geometry size");
    const view = new DataView(new Uint8Array(bytes).buffer);
    const pair = [view.getFloat64(0, true), view.getFloat64(8, true)];
    if (!pair.every(Number.isFinite)) throw new Error("Invalid AX geometry numbers");
    return pair;
}
if (typeof module !== "undefined") module.exports = {clipElement, optionalBoolean, doublePair};
function run(argv) {
    ObjC.import("CoreGraphics");
    ObjC.import("ApplicationServices");
    ObjC.import("AppKit");
    function permissions() {
        return {accessibilityTrusted: Boolean($.AXIsProcessTrusted()),
            postEventAllowed: optionalBoolean(() => $.CGPreflightPostEventAccess())};
    }
    if (argv[0] === "permissions") return JSON.stringify(permissions());
    const windowInfo = $.CGWindowListCopyWindowInfo(1, 0);
    const onScreen = ObjC.deepUnwrap(ObjC.castRefToObject(windowInfo));
    if (!Array.isArray(onScreen)) throw new Error("CoreGraphics window list did not bridge to an array");
    if (argv[0] === "inventory") {
        const seen = new Set();
        const windows = onScreen.filter(info => info.kCGWindowLayer === 0 && !seen.has(info.kCGWindowOwnerPID) && seen.add(info.kCGWindowOwnerPID));
        return JSON.stringify(windows.slice(0, 10).map(info => {
            const app = $.NSRunningApplication.runningApplicationWithProcessIdentifier(info.kCGWindowOwnerPID);
            let bundle = null, executable = null;
            try { bundle = ObjC.unwrap(app.bundleIdentifier); executable = ObjC.unwrap(app.executableURL.path); } catch (_) {}
            return {pid: info.kCGWindowOwnerPID, name: info.kCGWindowOwnerName,
                bundle: bundle, executable: executable};
        }));
    }
    if (argv[0] === "bridge-self-test") {
        if (!onScreen.every(info => info && typeof info.kCGWindowOwnerPID === "number" &&
            info.kCGWindowBounds && typeof info.kCGWindowBounds.X === "number")) {
            throw new Error("CoreGraphics window dictionaries did not bridge correctly");
        }
        return JSON.stringify({permissions: permissions(), windowArray: true, windowCount: onScreen.length});
    }
    const apps = $.NSWorkspace.sharedWorkspace.runningApplications;
    let processId = null;
    if (/^pid:[0-9]+$/.test(argv[1])) {
        const requested = Number(argv[1].slice(4));
        if (!onScreen.some(info => info.kCGWindowOwnerPID === requested)) throw new Error("Requested approval process is not on screen");
        processId = requested;
    }
    for (let index = 0; index < apps.count; index++) {
        const app = apps.objectAtIndex(index);
        if (ObjC.unwrap(app.localizedName) === argv[1]) {
            if (processId !== null) throw new Error("Ambiguous approval process");
            processId = Number(app.processIdentifier);
        }
    }
    if (processId === null) throw new Error("Approval process absent");
    const application = $.AXUIElementCreateApplication(processId);
    $.AXUIElementSetMessagingTimeout(application, 2);
    function attribute(element, name) {
        const output = Ref();
        const error = $.AXUIElementCopyAttributeValue(element, $(name), output);
        if (error !== 0) return null;
        return ObjC.castRefToObject(output[0]);
    }
    function scalar(element, name, fallback) {
        const value = attribute(element, name);
        return value === null ? fallback : ObjC.unwrap(value);
    }
    function children(element, name) {
        const values = attribute(element, name);
        if (values === null) return [];
        const result = [];
        for (let index = 0; index < values.count; index++) result.push(values.objectAtIndex(index));
        return result;
    }
    function geometry(element, name, type) {
        const value = attribute(element, name);
        if (value === null) return null;
        // Both supported macOS architectures represent CGPoint/CGSize as two
        // little-endian CGFloat doubles. A void* output has no JXA Ref type metadata.
        const storage = $.NSMutableData.dataWithLength(16);
        if (!$.AXValueGetValue(value, type, storage.mutableBytes)) return null;
        return doublePair(ObjC.unwrap(storage.base64EncodedStringWithOptions(0)));
    }
    function windowOnScreen(position, size) {
        if (!position || !size) return false;
        return onScreen.some(info => {
            const rect = info.kCGWindowBounds;
            return info.kCGWindowOwnerPID === processId && rect &&
                Math.abs(rect.X-position[0]) <= 2 && Math.abs(rect.Y-position[1]) <= 2 &&
                Math.abs(rect.Width-size[0]) <= 2 && Math.abs(rect.Height-size[1]) <= 2;
        });
    }
    const references = {};
    let count = 0;
    function model(element, path, parentVisible, viewport, isWindow) {
        if (++count > 1600 || path.length > 16) throw new Error("Approval UI tree exceeds bound");
        references[path.join("/")] = element;
        const position = geometry(element, "AXPosition", 1);
        const size = geometry(element, "AXSize", 2);
        const role = scalar(element, "AXRole", "");
        const hidden = scalar(element, "AXHidden", false);
        let visible = parentVisible && hidden !== true;
        if (isWindow) visible = hidden !== true && windowOnScreen(position, size);
        let clip = viewport;
        if (position && size && size[0] > 0 && size[1] > 0) {
            const clipped = clipElement(position, size, viewport, role === "AXButton");
            visible = visible && clipped.visible;
            clip = clipped.clip;
        } else if (role === "AXButton") visible = false;
        const subrole = scalar(element, "AXSubrole", "");
        const value = subrole === "AXSecureTextField" ? null : scalar(element, "AXValue", null);
        return {
            role: role, subrole: subrole,
            name: scalar(element, "AXTitle", null) || scalar(element, "AXDescription", null), value: typeof value === "string" ? value : null,
            enabled: scalar(element, "AXEnabled", false) === true, visible: visible,
            position: position, size: size,
            children: children(element, "AXChildren").map((child, index) => model(child, path.concat(index), visible, clip, false))
        };
    }
    const windows = children(application, "AXWindows");
    if (argv[1] === "System Settings" && windows.length === 0) throw new Error("System Settings AX windows unavailable");
    const tree = {role: "AXApplication", children: windows.map((win, index) => model(win, [index], true, null, true))};
    if (argv[0] === "snapshot") return JSON.stringify(tree);
    if (!["click", "scroll", "authenticate"].includes(argv[0])) throw new Error("Unknown approval UI action");
    const expected = JSON.parse(argv[2]);
    function at(path) { let node = tree; for (const index of path) node = node.children[index]; return node; }
    const container = at(expected.container_path);
    if (JSON.stringify(container) !== JSON.stringify(expected.container)) throw new Error("Approval warning changed; no click");
    const button = at(expected.button_path);
    if (argv[0] === "authenticate") {
        const owner = $.NSRunningApplication.runningApplicationWithProcessIdentifier(processId);
        if (!owner || !owner.executableURL || !ObjC.unwrap(owner.executableURL.path).startsWith("/System/") ||
            button.name !== "OK" || !button.visible || !button.enabled) throw new Error("Unexpected system authentication control");
        const field = at(expected.password_path);
        if (field.subrole !== "AXSecureTextField" || !field.visible || !field.enabled) throw new Error("System password field unavailable");
        const environment = $.NSProcessInfo.processInfo.environment;
        const root = ObjC.unwrap(environment.objectForKey("RUNNER_TEMP"));
        if (ObjC.unwrap(environment.objectForKey("GITHUB_ACTIONS")) !== "true" || !argv[3].startsWith(root + "/approval-credential-")) throw new Error("Authentication is restricted to the disposable runner");
        const password = $.NSString.stringWithContentsOfFileEncodingError(argv[3], $.NSUTF8StringEncoding, Ref());
        if (!password || password.length < 24 || password.length > 128) throw new Error("Invalid runner credential file");
        if ($.AXUIElementSetAttributeValue(references[expected.password_path.join("/")], $("AXValue"), password) !== 0) throw new Error("Cannot enter runner credential");
        if ($.AXUIElementPerformAction(references[expected.button_path.join("/")], $("AXPress")) !== 0) throw new Error("Cannot confirm runner authentication");
        return JSON.stringify({authenticationSubmitted: true});
    }
    if (argv[0] === "scroll") {
        if (button.role !== "AXButton" || button.name !== expected.label || !button.enabled) throw new Error("Approval scroll target is unavailable");
        for (let length = expected.button_path.length - 1; length > 0; length--) {
            const path = expected.button_path.slice(0, length);
            const area = at(path);
            if (area.role !== "AXScrollArea") continue;
            const bars = area.children.map((node, index) => ({node, path: path.concat(index)}))
                .filter(item => item.node.role === "AXScrollBar" && item.node.visible && item.node.enabled &&
                    scalar(references[item.path.join("/")], "AXOrientation", "") === "AXVerticalOrientation");
            if (bars.length !== 1 || !button.position || !area.position) throw new Error("Ambiguous approval scroll area");
            const bar = references[bars[0].path.join("/")];
            const current = scalar(bar, "AXValue", null);
            if (typeof current !== "number" || current < 0 || current > 1) throw new Error("Unexpected approval scrollbar range");
            const value = button.position[1] > area.position[1] ? 1 : 0;
            const result = $.AXUIElementSetAttributeValue(bar, $("AXValue"), $(value));
            if (result !== 0) throw new Error("Approval scrollbar failed: " + result);
            return JSON.stringify({scrolled: expected.label, scrollbar: bars[0].path, value: value});
        }
        throw new Error("No containing approval scroll area");
    }
    if (button.role !== "AXButton" || button.name !== expected.label || !button.visible || !button.enabled) throw new Error("Approval button is unavailable");
    const result = $.AXUIElementPerformAction(references[expected.button_path.join("/")], $("AXPress"));
    if (result !== 0) throw new Error("Approval press failed: " + result);
    return JSON.stringify({clicked: expected.label, button_path: expected.button_path, container_path: expected.container_path});
}
