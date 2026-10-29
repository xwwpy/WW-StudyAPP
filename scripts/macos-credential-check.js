// Disposable runner preflight. Never return credential contents or exception text.
function run(argv) {
    ObjC.import("Foundation");
    ObjC.import("OpenDirectory");
    let phase = "boundary";
    try {
        const environment = $.NSProcessInfo.processInfo.environment;
        const root = ObjC.unwrap(environment.objectForKey("RUNNER_TEMP"));
        if (ObjC.unwrap(environment.objectForKey("GITHUB_ACTIONS")) !== "true" || ObjC.unwrap($.NSUserName()) !== "runner" ||
            !/^studyapp_[0-9a-f]{12}$/.test(argv[0]) || !argv[1].startsWith(root + "/approval-credential-")) throw new Error();
        const password = $.NSString.stringWithContentsOfFileEncodingError(argv[1], $.NSUTF8StringEncoding, Ref());
        phase = "local-node";
        const error = Ref();
        const node = $.ODNode.nodeWithSessionTypeError($.ODSession.defaultSession, $.kODNodeTypeLocalNodes, error);
        phase = "record";
        const record = node.recordWithRecordTypeNameAttributesError($.kODRecordTypeUsers, $(argv[0]), null, error);
        phase = "verify-password";
        const verified = Boolean(record.verifyPasswordError(password, error));
        let code = null, domain = null;
        if (!verified && error[0]) {
            code = Number(error[0].code);
            domain = ObjC.unwrap(error[0].domain);
        }
        return JSON.stringify({verified: verified, code: code, domain: domain});
    } catch (_) {
        return JSON.stringify({verified: false, bridgePhase: phase});
    }
}
