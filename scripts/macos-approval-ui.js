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
function asciiKey(char) {
    const codes = {a: 0, b: 11, c: 8, d: 2, e: 14, f: 3, s: 1, t: 17, u: 32, y: 16, p: 35,
        "0": 29, "1": 18, "2": 19, "3": 20, "4": 21, "5": 23, "6": 22, "7": 26, "8": 28, "9": 25};
    if (char === "_") return [27, 131072];
    if (Object.prototype.hasOwnProperty.call(codes, char)) return [codes[char], 0];
    throw new Error("Unsupported disposable credential character");
}
if (typeof module !== "undefined") module.exports = {clipElement, optionalBoolean, doublePair, asciiKey};
function run(argv) {
    ObjC.import("CoreGraphics");
    ObjC.import("ApplicationServices");
    ObjC.import("AppKit");
    if (typeof $.CGPreflightPostEventAccess !== "function") {
        ObjC.bindFunction("CGPreflightPostEventAccess", ["bool", []]);
    }
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
        const windows = onScreen.filter(info => !seen.has(info.kCGWindowOwnerPID) && seen.add(info.kCGWindowOwnerPID));
        if (windows.length > 10) throw new Error("Too many visible owners to establish unique authentication");
        return JSON.stringify(windows.map(info => {
            const app = $.NSRunningApplication.runningApplicationWithProcessIdentifier(info.kCGWindowOwnerPID);
            let bundle = null, executable = null;
            try { bundle = ObjC.unwrap(app.bundleIdentifier); executable = ObjC.unwrap(app.executableURL.path); } catch (_) {}
            return {pid: info.kCGWindowOwnerPID, name: info.kCGWindowOwnerName,
                layer: info.kCGWindowLayer, bundle: bundle, executable: executable};
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
        let phase = "permission";
        const focusObservations = [];
        try {
        if (!$.CGPreflightPostEventAccess()) throw new Error("System event permission unavailable");
        phase = "identity";
        const owner = $.NSRunningApplication.runningApplicationWithProcessIdentifier(processId);
        if (!owner || !owner.executableURL || !ObjC.unwrap(owner.executableURL.path).startsWith("/System/") ||
            button.name !== "OK" || !button.visible || !button.enabled) throw new Error("Unexpected system authentication control");
        const field = at(expected.password_path);
        if (field.subrole !== "AXSecureTextField" || !field.visible || !field.enabled) throw new Error("System password field unavailable");
        const username = at(expected.username_path);
        if (username.role !== "AXTextField" || username.subrole === "AXSecureTextField" || username.value !== "runner" ||
            !username.visible || !username.enabled || !/^studyapp_[0-9a-f]{12}$/.test(expected.test_username)) throw new Error("Unexpected disposable administrator identity");
        const environment = $.NSProcessInfo.processInfo.environment;
        const root = ObjC.unwrap(environment.objectForKey("RUNNER_TEMP"));
        if (ObjC.unwrap(environment.objectForKey("GITHUB_ACTIONS")) !== "true" || !argv[3].startsWith(root + "/approval-credential-")) throw new Error("Authentication is restricted to the disposable runner");
        phase = "credential-file";
        const password = $.NSString.stringWithContentsOfFileEncodingError(argv[3], $.NSUTF8StringEncoding, Ref());
        if (!password || password.length < 24 || password.length > 128) throw new Error("Invalid runner credential file");
        function key(code, flags) {
            for (const down of [true, false]) {
                const event = $.CGEventCreateKeyboardEvent(null, code, down);
                $.CGEventSetFlags(event, flags);
                $.CGEventPost(0, event);
            }
        }
        function typeInto(control, text, fieldName) {
            let dismissedAutocomplete = false;
            phase = fieldName + "-focus";
            if ($.AXUIElementSetAttributeValue(control, $("AXFocused"), $(true)) !== 0) throw new Error("Cannot focus system credential field");
            $.NSThread.sleepForTimeInterval(0.1);
            function requireFocus() {
                phase = fieldName + "-focus";
                const focused = attribute($.AXUIElementCreateSystemWide(), "AXFocusedUIElement");
                const path = fieldName === "password" ? expected.password_path : expected.username_path;
                let fresh = children(application, "AXWindows")[path[0]];
                for (const index of path.slice(1)) fresh = children(fresh, "AXChildren")[index];
                const pid = Ref();
                const pidKnown = focused !== null && $.AXUIElementGetPid(focused, pid) === 0;
                const observation = {field: fieldName, local: scalar(control, "AXFocused", false),
                    globalPresent: focused !== null, globalRole: focused === null ? null : scalar(focused, "AXRole", null),
                    globalSubrole: focused === null ? null : scalar(focused, "AXSubrole", null),
                    sameProcess: pidKnown && pid[0] === processId,
                    matchesOriginal: focused !== null && Boolean($.CFEqual(focused, control)),
                    matchesFresh: focused !== null && Boolean($.CFEqual(focused, fresh))};
                if (fieldName === "password" && !dismissedAutocomplete && observation.local && observation.sameProcess &&
                    observation.globalRole === "AXTable" && !observation.matchesOriginal) {
                    const label = scalar(focused, "AXTitle", null) || scalar(focused, "AXDescription", "");
                    let ancestor = focused, popup = null;
                    for (let depth = 0; ancestor !== null && depth < 6; depth++) {
                        if (scalar(ancestor, "AXRole", "") === "AXWindow") { popup = ancestor; break; }
                        ancestor = attribute(ancestor, "AXParent");
                    }
                    if (/^Auto-Complete [0-9]+ Results?$/.test(label) && popup !== null &&
                        !$.CFEqual(popup, references[expected.container_path.join("/")])) {
                        dismissedAutocomplete = true;
                        key(53, 0);
                        $.NSThread.sleepForTimeInterval(0.2);
                        control = children(application, "AXWindows")[path[0]];
                        for (const index of path.slice(1)) control = children(control, "AXChildren")[index];
                        if (scalar(control, "AXRole", "") !== "AXTextField" || scalar(control, "AXSubrole", "") !== "AXSecureTextField" ||
                            JSON.stringify(geometry(control, "AXPosition", 1)) !== JSON.stringify(field.position) ||
                            JSON.stringify(geometry(control, "AXSize", 2)) !== JSON.stringify(field.size)) throw new Error("Secure field changed after popup dismissal");
                        focusObservations.push({field: "password", action: "escape-autocomplete"});
                        return requireFocus();
                    }
                }
                if (!observation.local || !observation.matchesOriginal) {
                    observation.ancestors = [];
                    let ancestor = focused;
                    for (let depth = 0; ancestor !== null && depth < 6; depth++) {
                        observation.ancestors.push({role: scalar(ancestor, "AXRole", null), subrole: scalar(ancestor, "AXSubrole", null),
                            position: geometry(ancestor, "AXPosition", 1), size: geometry(ancestor, "AXSize", 2),
                            isField: Boolean($.CFEqual(ancestor, fresh)), isWindow: Boolean($.CFEqual(ancestor, references[expected.container_path.join("/")]))});
                        ancestor = attribute(ancestor, "AXParent");
                    }
                    observation.children = focused === null ? [] : children(focused, "AXChildren").slice(0, 8).map(child => ({role: scalar(child, "AXRole", null), subrole: scalar(child, "AXSubrole", null)}));
                    focusObservations.push(observation);
                }
                if (scalar(control, "AXFocused", false) !== true || focused === null || !$.CFEqual(focused, control)) throw new Error("System credential focus changed");
            }
            requireFocus();
            key(0, 1048576); // Select all through the normal Command-A shortcut.
            requireFocus();
            key(51, 0);
            for (const char of text) {
                requireFocus();
                phase = fieldName + "-keypost";
                const stroke = asciiKey(char);
                key(stroke[0], stroke[1]);
                $.NSThread.sleepForTimeInterval(0.01);
            }
        }
        typeInto(references[expected.username_path.join("/")], expected.test_username, "username");
        phase = "username-delivery";
        if (scalar(references[expected.username_path.join("/")], "AXValue", "") !== expected.test_username) throw new Error("System username keyboard input was not delivered");
        typeInto(references[expected.password_path.join("/")], ObjC.unwrap(password), "password");
        $.NSThread.sleepForTimeInterval(0.4);
        phase = "button-owner";
        const control = references[expected.button_path.join("/")];
        const position = geometry(control, "AXPosition", 1), size = geometry(control, "AXSize", 2);
        if (!position || !size || scalar(control, "AXEnabled", false) !== true) throw new Error("Authentication control changed");
        const point = [position[0] + size[0] / 2, position[1] + size[1] / 2];
        const currentWindows = ObjC.deepUnwrap(ObjC.castRefToObject($.CGWindowListCopyWindowInfo(1, 0)));
        const front = currentWindows.find(info => {
            const r = info.kCGWindowBounds;
            return info.kCGWindowAlpha > 0 && r && point[0] >= r.X && point[0] < r.X + r.Width && point[1] >= r.Y && point[1] < r.Y + r.Height;
        });
        if (!front || front.kCGWindowOwnerPID !== processId) throw new Error("Authentication control is obscured");
        phase = "button-default";
        const defaultButton = attribute(references[expected.container_path.join("/")], "AXDefaultButton");
        if (defaultButton === null || !$.CFEqual(defaultButton, control)) throw new Error("System default action is not the matched OK button");
        let secure = children(application, "AXWindows")[expected.password_path[0]];
        for (const index of expected.password_path.slice(1)) secure = children(secure, "AXChildren")[index];
        const focused = attribute($.AXUIElementCreateSystemWide(), "AXFocusedUIElement");
        if (focused === null || !$.CFEqual(focused, secure) || scalar(secure, "AXSubrole", "") !== "AXSecureTextField") throw new Error("Secure focus changed before Return");
        phase = "button-event";
        key(36, 0);
        return JSON.stringify({authenticationSubmitted: true, postEventAllowed: true,
            submission: "default-return", defaultButtonMatched: true, buttonPoint: point, focusObservations: focusObservations});
        } catch (_) {
            return JSON.stringify({authenticationSubmitted: false, phase: phase, focusObservations: focusObservations});
        }
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
