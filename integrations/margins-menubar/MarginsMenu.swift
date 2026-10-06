import AppKit
import AVFoundation
import SwiftUI

@MainActor
final class MenuRecorder: ObservableObject {
    private let preferences = UserDefaults(suiteName: ProcessInfo.processInfo.environment["MARGINS_MENU_SETTINGS_DOMAIN"] ?? "com.byenzyme.margins.menu") ?? .standard
    @Published var setupComplete = false { didSet { preferences.set(setupComplete, forKey: "setupComplete") } }
    @Published var title = "Meeting"
    @Published var remote = ProcessInfo.processInfo.environment["MARGINS_MENU_REMOTE"] ?? "" { didSet { preferences.set(remote, forKey: "remote") } }
    @Published var workspace = ProcessInfo.processInfo.environment["MARGINS_MENU_WORKSPACE"] ?? "" { didSet { preferences.set(workspace, forKey: "workspace") } }
    @Published var status = "Checking recorder…"
    @Published var state = "ready"
    @Published var error: String?
    @Published var sessionID: String?
    @Published var connectedWorkspaceName: String?
    @Published var microphones: [String] = []
    @Published var microphoneChoice = ""
    @Published var microphoneBusy = false
    @Published var microphonePermission = "unknown"
    private(set) var connectedOrigin: String?

    private var pairingServer: MenuPairingServer?
    private var grantToken: String?
    private var grantExpiresAt: Int64?
    private var bbMachineCredential: String?
    private var bridgeToken: String?
    private var bridgePID: Int32?
    private var pairDirectory: URL?
    private let bridgePort = 18765
    private let bridgeOrigin = "http://127.0.0.1:18766"

    init() {
        if ProcessInfo.processInfo.environment["MARGINS_MENU_REMOTE"] == nil {
            remote = preferences.string(forKey: "remote") ?? ""
        }
        if ProcessInfo.processInfo.environment["MARGINS_MENU_WORKSPACE"] == nil {
            workspace = preferences.string(forKey: "workspace") ?? ""
        }
        setupComplete = preferences.bool(forKey: "setupComplete") &&
            preferences.string(forKey: "destination") == "BB project"
        microphoneChoice = preferences.string(forKey: "microphoneName") ?? ""
        refreshMicrophones()
        do { pairingServer = try MenuPairingServer(recorder: self) }
        catch { self.error = "bb connection is unavailable: \(error.localizedDescription)" }
        Task { await refresh() }
        Task {
            while !Task.isCancelled {
                // Poll quickly while starting so "Recording" appears as soon
                // as the microphone delivers audio.
                try? await Task.sleep(for: state == "getting_ready" ? .milliseconds(250) : .seconds(2))
                await refresh()
            }
        }
    }

    var active: Bool { ["starting", "getting_ready", "recording", "paused", "saving", "finalizing"].contains(state) }

    var defaultMicrophone: String {
        AVCaptureDevice.default(for: .audio)?.localizedName ?? "Unavailable"
    }

    private func refreshMicrophones() {
        microphones = Array(Set(AVCaptureDevice.DiscoverySession(
            deviceTypes: [.microphone], mediaType: .audio, position: .unspecified
        ).devices.map(\.localizedName))).sorted()
    }

    func chooseMicrophone(_ name: String) async {
        guard !active && !microphoneBusy else { return }
        refreshMicrophones()
        guard name.isEmpty || microphones.contains(name) else {
            error = "That microphone is unavailable. Choose another input."
            return
        }
        microphoneBusy = true
        defer { microphoneBusy = false }
        do {
            if bridgeToken != nil {
                let selection: Any = name.isEmpty ? NSNull() as Any : name as Any
                _ = try await bridgeRequest("/v1/microphone", body: ["deviceName": selection])
            }
            microphoneChoice = name
            preferences.set(name, forKey: "microphoneName")
            error = nil
            await refresh()
        } catch { self.error = error.localizedDescription }
    }

    func requestMicrophonePermission() async {
        guard bridgeToken != nil && !active && !microphoneBusy else { return }
        microphoneBusy = true
        do {
            _ = try await bridgeRequest("/v1/microphone-permission", body: [:])
            error = nil
        } catch {
            self.error = "Margins Capture could not use the microphone. Check its access in System Settings."
        }
        microphoneBusy = false
        await refresh()
    }

    func openMicrophoneSettings() {
        guard let url = URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone") else { return }
        NSWorkspace.shared.open(url)
    }

    func chooseProject() async {
        error = nil
        preferences.set("BB project", forKey: "destination")
        do {
            try await connectBridge()
            setupComplete = true
            await refresh()
        } catch { self.error = error.localizedDescription }
    }

    func changeDestination() async {
        guard !active else { return }
        if bridgeToken != nil { await disconnect() }
        setupComplete = false
        error = nil
    }

    func connectGrant(_ grant: MenuGrant, origin: String) async throws -> [String: Any] {
        guard !active else { throw MenuError("Finish the current meeting before changing Workspace") }
        guard grant.expiresAt > Int64(Date().timeIntervalSince1970 * 1_000),
              let service = URL(string: grant.serviceUrl),
              grant.serviceUrl == origin + "/api/v1/plugins/margins/http/menu/relay",
              service.scheme == "https" || service.host == "127.0.0.1" || service.host == "localhost" else {
            throw MenuError("The bb capture grant has an invalid destination")
        }
        let machineCredential = try machineCredential(for: origin)
        var verify = URLRequest(url: URL(string: origin + "/api/v1/plugins/margins/http/menu/verify")!)
        verify.httpMethod = "POST"
        verify.setValue("Bearer \(grant.token)", forHTTPHeaderField: "Authorization")
        if let machineCredential { verify.setValue(machineCredential, forHTTPHeaderField: "x-bb-connect-machine") }
        verify.timeoutInterval = 10
        let (data, response) = try await URLSession.shared.data(for: verify)
        guard let http = response as? HTTPURLResponse, http.statusCode == 200,
              let result = try JSONSerialization.jsonObject(with: data) as? [String: Any],
              result["ok"] as? Bool == true,
              result["origin"] as? String == origin,
              result["workspaceId"] as? String == grant.workspaceId,
              result["instanceId"] as? String == grant.instanceId else {
            throw MenuError("bb did not verify this Workspace grant")
        }
        if bridgeToken != nil { await disconnect() }
        remote = "http://127.0.0.1:18764/api/v1/plugins/margins/http/menu/relay"
        workspace = grant.workspaceId
        preferences.set("BB project", forKey: "destination")
        connectedOrigin = origin
        bbMachineCredential = machineCredential
        grantToken = grant.token
        grantExpiresAt = grant.expiresAt
        try await connectBridge(origin: origin, remoteToken: grant.token)
        connectedWorkspaceName = grant.workspaceName
        setupComplete = true
        let snapshot = try await bridgeRequest("/v1/status")
        return ["token": bridgeToken ?? "", "instanceId": grant.instanceId,
                "workspaceId": grant.workspaceId, "status": snapshot]
    }

    private func machineCredential(for origin: String) throws -> String? {
        guard let url = URL(string: origin), let host = url.host else { throw MenuError("Invalid bb origin") }
        if url.scheme == "http" && ["127.0.0.1", "localhost"].contains(host) { return nil }
        guard url.scheme == "https" else { throw MenuError("Connect from an HTTPS bb server") }
        if host != "getbb.app" && !host.hasSuffix(".getbb.app") { return nil }
        let path = FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent(".bb-machines/\(host)/config.json")
        let data = try Data(contentsOf: path)
        guard let config = try JSONSerialization.jsonObject(with: data) as? [String: Any],
              config["serverUrl"] as? String == origin,
              let credential = config["machineCredential"] as? String,
              credential.hasPrefix("bbcm_"), credential.count >= 32 else {
            throw MenuError("Enroll this Mac as a bb machine before connecting Margins Menu")
        }
        return credential
    }

    func forwardCaptureRelay(_ body: Data, token: String) async throws -> [String: Any] {
        guard let origin = connectedOrigin, token == grantToken,
              let expires = grantExpiresAt,
              expires > Int64(Date().timeIntervalSince1970 * 1_000) else {
            throw MenuError("The bb capture grant expired or was revoked")
        }
        var request = URLRequest(url: URL(string: origin + "/api/v1/plugins/margins/http/menu/relay")!)
        request.httpMethod = "POST"
        request.httpBody = body
        request.timeoutInterval = 30
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        if let bbMachineCredential { request.setValue(bbMachineCredential, forHTTPHeaderField: "x-bb-connect-machine") }
        let (responseBody, response) = try await URLSession.shared.data(for: request)
        guard let http = response as? HTTPURLResponse, http.statusCode == 200,
              let relay = try JSONSerialization.jsonObject(with: responseBody) as? [String: Any],
              relay["status"] as? Int != nil, relay["bodyBase64"] as? String != nil else {
            throw MenuError("bb could not relay the capture upload")
        }
        return relay
    }

    func refresh() async {
        if microphoneBusy { return }
        // The picker is locked during a meeting; don't rescan devices on the fast start poll.
        if !active { refreshMicrophones() }
        if !setupComplete {
            state = "ready"
            status = "Choose where meetings live"
            return
        }
        do {
            if let grantToken, let connectedOrigin, let grantExpiresAt,
               grantExpiresAt < Int64(Date().timeIntervalSince1970 * 1_000) + 600_000 {
                let renewed = try await request(connectedOrigin + "/api/v1/plugins/margins/http/menu/renew",
                                                token: grantToken, body: [:])
                guard renewed["ok"] as? Bool == true,
                      let expires = renewed["expiresAt"] as? Int64 else {
                    throw MenuError("bb capture access expired; reconnect from Meetings")
                }
                self.grantExpiresAt = expires
            }
            if bridgeToken != nil {
                let snapshot = try await bridgeRequest("/v1/status")
                state = snapshot["state"] as? String ?? "ready"
                if !active {
                    let permission = try await bridgeRequest("/v1/microphone-permission")
                    microphonePermission = permission["status"] as? String ?? "unknown"
                }
                sessionID = snapshot["sessionId"] as? String
                bridgePID = (snapshot["pid"] as? NSNumber)?.int32Value
                let mic = (snapshot["microphoneSamples"] as? NSNumber)?.intValue ?? 0
                let system = (snapshot["systemSamples"] as? NSNumber)?.intValue ?? 0
                status = state == "getting_ready" ? "Starting… don't speak yet"
                    : "\(state) · mic \(mic) · system \(system) samples"
                error = snapshot["error"] as? String
            } else {
                state = "ready"
                microphonePermission = "unknown"
                status = remote.contains("/plugins/margins/http/menu/relay")
                    ? "Open Meetings in bb to reconnect" : "Connect the Mac bridge to the BB project"
            }
        } catch {
            status = "Mac bridge is unavailable"
            if active { self.error = error.localizedDescription }
        }
    }

    func start() async {
        error = nil
        do {
            if !microphoneChoice.isEmpty && !microphones.contains(microphoneChoice) {
                throw MenuError("\(microphoneChoice) is unavailable. Choose another microphone before recording.")
            }
            if bridgeToken == nil {
                if remote.contains("/plugins/margins/http/menu/relay") && grantToken == nil {
                    throw MenuError("Reconnect from the Meetings page in bb")
                }
                try await connectBridge(origin: connectedOrigin, remoteToken: grantToken)
            }
            _ = try await bridgeRequest("/v1/start", body: ["title": title])
            await refresh()
        } catch { self.error = error.localizedDescription }
    }

    func control(_ action: String) async {
        error = nil
        do {
            _ = try await bridgeRequest("/v1/\(action)", body: [:])
            await refresh()
        } catch { self.error = error.localizedDescription }
    }

    func connectBridge(origin browserOrigin: String? = nil, remoteToken: String? = nil) async throws {
        guard !remote.trimmingCharacters(in: .whitespaces).isEmpty,
              !workspace.trimmingCharacters(in: .whitespaces).isEmpty else {
            throw MenuError("Enter a remote and Workspace")
        }
        let bundled = Bundle.main.bundleURL.appendingPathComponent("Contents/Helpers/Margins Capture.app").path
        guard let bridgeApp = ProcessInfo.processInfo.environment["MARGINS_MENU_BRIDGE_APP"] ??
                (FileManager.default.fileExists(atPath: bundled) ? bundled : nil) else {
            throw MenuError("Install the Margins capture helper to connect this Workspace")
        }
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent("margins-menu-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: false,
                                                attributes: [.posixPermissions: 0o700])
        pairDirectory = directory
        let pairFile = directory.appendingPathComponent("pair-code")
        let tokenFile = directory.appendingPathComponent("remote-token")
        let audioDirectory = directory.appendingPathComponent("local-audio")
        var args = ["-n"]
        for (source, target) in [("MARGINS_MENU_SSH_REMOTE_BINARY", "MARGINS_SSH_REMOTE_BINARY"),
                                 ("MARGINS_MENU_SSH_REMOTE_DATA_DIR", "MARGINS_SSH_REMOTE_DATA_DIR"),
                                 ("MARGINS_MENU_HOME", "MARGINS_HOME"),
                                 ("MARGINS_MENU_TRANSFER_DIR", "MARGINS_TRANSFER_DIR")] {
            if let value = ProcessInfo.processInfo.environment[source] { args += ["--env", "\(target)=\(value)"] }
        }
        args += ["-a", bridgeApp, "--args", "native-bridge", "--remote", remote,
                 "--workspace", workspace, "--origin", browserOrigin ?? bridgeOrigin, "--port", String(bridgePort),
                 "--pair-code-file", pairFile.path, "--local-audio-dir", audioDirectory.path]
        if let browserOrigin, browserOrigin != bridgeOrigin { args += ["--menu-origin", bridgeOrigin] }
        if let remoteToken {
            guard FileManager.default.createFile(atPath: tokenFile.path, contents: Data(remoteToken.utf8),
                                                 attributes: [.posixPermissions: 0o600]) else {
                throw MenuError("Could not stage the bb capture grant")
            }
            args += ["--remote-token-file", tokenFile.path]
        }
        let micDevice = microphoneChoice.isEmpty
            ? ProcessInfo.processInfo.environment["MARGINS_MENU_MIC_DEVICE"] : microphoneChoice
        if let micDevice, !micDevice.isEmpty {
            args += ["--mic-device", micDevice]
        }
        let launcher = Process()
        launcher.executableURL = URL(fileURLWithPath: "/usr/bin/open")
        launcher.arguments = args
        try launcher.run()
        launcher.waitUntilExit()
        guard launcher.terminationStatus == 0 else { throw MenuError("Could not launch the Mac bridge") }
        var code: String?
        for _ in 0..<100 {
            if let value = try? String(contentsOf: pairFile, encoding: .utf8), !value.isEmpty {
                code = value.trimmingCharacters(in: .whitespacesAndNewlines)
                break
            }
            try await Task.sleep(for: .milliseconds(100))
        }
        guard let code else { throw MenuError("Mac bridge did not publish a pairing code") }
        let response = try await request("http://127.0.0.1:\(bridgePort)/v1/pair", token: nil,
                                         origin: bridgeOrigin, body: ["code": code])
        guard response["workspaceId"] as? String == workspace,
              let token = response["token"] as? String else {
            throw MenuError("Bridge pairing returned a different Workspace")
        }
        bridgeToken = token
        try? FileManager.default.removeItem(at: pairFile)
        await refresh()
    }

    func disconnect() async {
        guard !active else { error = "Stop and save before disconnecting"; return }
        if let bridgePID { kill(bridgePID, SIGTERM) }
        bridgeToken = nil
        grantToken = nil
        grantExpiresAt = nil
        bbMachineCredential = nil
        connectedOrigin = nil
        connectedWorkspaceName = nil
        bridgePID = nil
        microphonePermission = "unknown"
        if let pairDirectory { try? FileManager.default.removeItem(at: pairDirectory) }
        pairDirectory = nil
        sessionID = nil
        await refresh()
    }

    private func bridgeRequest(_ path: String, body: [String: Any]? = nil) async throws -> [String: Any] {
        guard let bridgeToken else { throw MenuError("Mac bridge is not paired") }
        return try await request("http://127.0.0.1:\(bridgePort)\(path)", token: bridgeToken,
                                 origin: bridgeOrigin, body: body)
    }

    private func request(_ endpoint: String, token: String?, origin: String? = nil,
                         body: [String: Any]? = nil) async throws -> [String: Any] {
        guard let url = URL(string: endpoint) else { throw MenuError("Invalid recorder URL") }
        var request = URLRequest(url: url)
        request.timeoutInterval = endpoint.hasSuffix("/v1/microphone-permission") && body != nil ? 100 : 10
        if let token { request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization") }
        if let connectedOrigin, endpoint.hasPrefix(connectedOrigin + "/"), let bbMachineCredential {
            request.setValue(bbMachineCredential, forHTTPHeaderField: "x-bb-connect-machine")
        }
        if let origin { request.setValue(origin, forHTTPHeaderField: "Origin") }
        if let body {
            request.httpMethod = "POST"
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
            request.httpBody = try JSONSerialization.data(withJSONObject: body)
        }
        let (data, response) = try await URLSession.shared.data(for: request)
        guard let http = response as? HTTPURLResponse else { throw MenuError("Recorder gave no HTTP response") }
        let value = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any] ?? [:]
        if !(200..<300).contains(http.statusCode) {
            let message = value["error"] as? String ?? "Recorder returned HTTP \(http.statusCode)"
            throw MenuError(message)
        }
        return value
    }
}

private struct MenuError: LocalizedError {
    let message: String
    init(_ message: String) { self.message = message }
    var errorDescription: String? { message }
}

private struct RecorderControls: View {
    @ObservedObject var recorder: MenuRecorder
    @State private var showManualConnection = false

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("Margins").font(.headline)
            if !recorder.setupComplete {
                Text("Where should meetings live?").font(.subheadline)
                Text("With bb").font(.subheadline)
                Text("Open Meetings in bb, set up a Workspace if asked, then click Connect. bb sends it here automatically.")
                    .font(.caption).foregroundStyle(.secondary)
                Divider()
                DisclosureGroup("Manual connection (advanced)", isExpanded: $showManualConnection) {
                    TextField("SSH alias or HTTPS URL", text: $recorder.remote)
                    TextField("Workspace ID", text: $recorder.workspace)
                    Button("Connect Workspace") { Task { await recorder.chooseProject() } }
                }
            } else {
                Text(recorder.connectedWorkspaceName ?? "BB project").font(.subheadline)
                Text(recorder.status).font(.caption).foregroundStyle(.secondary)
                Group {
                    Picker("Microphone", selection: Binding(
                        get: { recorder.microphoneChoice },
                        set: { choice in Task { await recorder.chooseMicrophone(choice) } }
                    )) {
                        Text("System default (\(recorder.defaultMicrophone))").tag("")
                        ForEach(recorder.microphones, id: \.self) { name in Text(name).tag(name) }
                        if !recorder.microphoneChoice.isEmpty && !recorder.microphones.contains(recorder.microphoneChoice) {
                            Text("\(recorder.microphoneChoice) (unavailable)").tag(recorder.microphoneChoice)
                        }
                    }
                    .disabled(recorder.active || recorder.microphoneBusy)
                    if recorder.microphonePermission == "not_determined" {
                        Button("Allow microphone") { Task { await recorder.requestMicrophonePermission() } }
                            .disabled(recorder.microphoneBusy)
                        Text("macOS will ask for Margins Capture access. No meeting starts yet.")
                            .font(.caption).foregroundStyle(.secondary)
                    } else if recorder.microphonePermission == "denied" || recorder.microphonePermission == "restricted" {
                        Text("Microphone access is blocked for Margins Capture.")
                            .font(.caption).foregroundStyle(.red)
                        Button("Open Microphone Settings") { recorder.openMicrophoneSettings() }
                    }
                }
                if let session = recorder.sessionID {
                    Text("Meeting: \(session)").font(.caption2).lineLimit(1).truncationMode(.middle)
                        .help(session)
                }
                Button(recorder.active ? "Finish meeting" : "Record meeting") {
                    Task {
                        if recorder.active { await recorder.control("stop") }
                        else { await recorder.start() }
                    }
                }
                .disabled(recorder.microphoneBusy || ["starting", "saving", "finalizing"].contains(recorder.state))
                Menu("More") {
                    if recorder.state == "recording" {
                        Button("Pause") { Task { await recorder.control("pause") } }
                    } else if recorder.state == "paused" {
                        Button("Resume") { Task { await recorder.control("resume") } }
                    }
                    Button("Change destination") { Task { await recorder.changeDestination() } }
                        .disabled(recorder.active)
                    Button("Quit Margins") {
                        Task {
                            await recorder.disconnect()
                            NSApp.terminate(nil)
                        }
                    }.disabled(recorder.active)
                }
            }
            if let error = recorder.error { Text(error).font(.caption).foregroundStyle(.red) }
        }
        .padding(14)
        .frame(width: 330)
    }
}

@main
struct MarginsMenuApp: App {
    @StateObject private var recorder = MenuRecorder()

    var body: some Scene {
#if MARGINS_MENU_TEST_WINDOW
        WindowGroup("Margins Menu Test Controls", id: "menu-test-controls") {
            RecorderControls(recorder: recorder)
        }
#else
        MenuBarExtra("Margins", systemImage: "waveform") { RecorderControls(recorder: recorder) }
            .menuBarExtraStyle(.window)
#endif
    }
}
