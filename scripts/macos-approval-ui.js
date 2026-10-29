// Remote runner only: snapshot accessibility containers and click an unchanged selection.
function run(argv) {
    const system = Application("System Events");
    const process = system.processes.byName(argv[1]);
    if (!process.exists()) throw new Error("Approval process absent");
    const references = {};
    let count = 0;
    function read(getter, fallback) { try { return getter(); } catch (_) { return fallback; } }
    function model(element, path, parentVisible, viewport, isWindow) {
        if (++count > 1600 || path.length > 16) throw new Error("Approval UI tree exceeds bound");
        references[path.join("/")] = element;
        const position = read(() => element.position(), null);
        const size = read(() => element.size(), null);
        const hidden = read(() => element.attributes.byName("AXHidden").value(), false);
        let visible = parentVisible && hidden !== true;
        if (isWindow) visible = read(() => element.visible(), false) === true;
        let clip = viewport;
        if (position && size && size[0] > 0 && size[1] > 0) {
            const rect = [position[0], position[1], position[0] + size[0], position[1] + size[1]];
            if (viewport) {
                const center = [position[0] + size[0] / 2, position[1] + size[1] / 2];
                visible = visible && center[0] >= viewport[0] && center[0] < viewport[2] && center[1] >= viewport[1] && center[1] < viewport[3];
                clip = [Math.max(rect[0], viewport[0]), Math.max(rect[1], viewport[1]), Math.min(rect[2], viewport[2]), Math.min(rect[3], viewport[3])];
            } else clip = rect;
        } else if (read(() => element.role(), "") === "AXButton") visible = false;
        const value = read(() => element.value(), null);
        return {
            role: read(() => element.role(), ""), subrole: read(() => element.subrole(), ""),
            name: read(() => element.name(), ""), value: typeof value === "string" ? value : null,
            enabled: read(() => element.enabled(), false) === true, visible: visible,
            position: position, size: size,
            children: read(() => element.uiElements(), []).map((child, index) => model(child, path.concat(index), visible, clip, false))
        };
    }
    const tree = {role: "AXApplication", children: process.windows().map((win, index) => model(win, [index], true, null, true))};
    if (argv[0] === "snapshot") return JSON.stringify(tree);
    if (argv[0] !== "click") throw new Error("Unknown approval UI action");
    const expected = JSON.parse(argv[2]);
    function at(path) { let node = tree; for (const index of path) node = node.children[index]; return node; }
    const container = at(expected.container_path);
    if (JSON.stringify(container) !== JSON.stringify(expected.container)) throw new Error("Approval warning changed; no click");
    const button = at(expected.button_path);
    if (button.role !== "AXButton" || button.name !== expected.label || !button.visible || !button.enabled) throw new Error("Approval button is unavailable");
    references[expected.button_path.join("/")].click();
    return JSON.stringify({clicked: expected.label, button_path: expected.button_path, container_path: expected.container_path});
}
