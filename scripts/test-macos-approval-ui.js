const assert = require("node:assert/strict");
const { clipElement } = require("./macos-approval-ui.js");
const group = clipElement([0, 0], [400, 1800], [0, 0, 400, 500], false);
assert.equal(group.visible, true);
assert.deepEqual(group.clip, [0, 0, 400, 500]);
assert.equal(clipElement([20, 420], [150, 30], group.clip, true).visible, true);
assert.equal(clipElement([20, 700], [150, 30], group.clip, true).visible, false);
assert.equal(clipElement([0, 600], [400, 200], group.clip, false).visible, false);
console.log("approval geometry passed");
