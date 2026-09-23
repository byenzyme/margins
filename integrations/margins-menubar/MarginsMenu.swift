import AppKit
import SwiftUI

private enum CaptureMode: String, CaseIterable, Identifiable {
    case mac = "On this Mac"
    case project = "BB project"
    var id: String { rawValue }
}

@MainActor
final class MenuRecorder: ObservableObject {
    @Published var mode: CaptureMode = .mac
    @Published var title = "Meeting"
    @Published var remote = ProcessInfo.processInfo.environment["MARGINS_MENU_REMOTE"] ?? ""
    @Published var workspace = ProcessInfo.processInfo.environment["MARGINS_MENU_WORKSPACE"] ?? ""
    @Published var status = "Checking recorder…"
    @Published var state = "ready"
    @Published var error: String?
    @Published var sessionID: String?
    @Published var localAudioPaths: [String] = []

    private var generation: Int?
    private var bridgeToken: String?
    private var bridgePID: Int32?
    private var pairDirectory: URL?
    private let bridgePort = 18765
    private let bridgeOrigin = "http://127.0.0.1:18766"

    init() {
        Task { await refresh() }
        Task {
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(2))
                await refresh()
            }
        }
    }

    var active: Bool { ["starting", "getting_ready", "recording", "paused", "saving", "finalizing"].contains(state) }

    func refresh() async {
        do {
            if mode == .mac {
                let discovery = try readDiscovery()
                let snapshot = try await request(discovery.baseURL + "/v1/live/snapshot", token: discovery.token)
                let session = snapshot["session"] as? [String: Any]
                sessionID = session?["session_id"] as? String
                generation = session?["generation"] as? Int
                state = session?["status"] as? String ?? "ready"
                let health = snapshot["health"] as? [String: Any]
                let system = (health?["system_audio_observed"] as? Bool) == true ? "system audio seen" : "waiting for system audio"
                let lines = (snapshot["rolling_transcript"] as? [[String: Any]])?.count ?? 0
                status = session == nil ? "Mac recorder ready" : "\(state) · \(system) · \(lines) transcript lines"
            } else if bridgeToken != nil {
                let snapshot = try await bridgeRequest("/v1/status")
                state = snapshot["state"] as? String ?? "ready"
                sessionID = snapshot["sessionId"] as? String
                bridgePID = (snapshot["pid"] as? NSNumber)?.int32Value
                localAudioPaths = snapshot["localAudioPaths"] as? [String] ?? []
                let mic = (snapshot["microphoneSamples"] as? NSNumber)?.intValue ?? 0
                let system = (snapshot["systemSamples"] as? NSNumber)?.intValue ?? 0
                status = "\(state) · mic \(mic) · system \(system) samples"
                error = snapshot["error"] as? String
            } else {
                state = "ready"
                status = "Connect the Mac bridge to the BB project"
            }
        } catch {
            status = mode == .mac ? "Mac recorder is unavailable" : "Mac bridge is unavailable"
            if active { self.error = error.localizedDescription }
        }
    }

    func start() async {
        error = nil
        do {
            if mode == .mac {
                let discovery = try readDiscovery()
                _ = try await request(discovery.baseURL + "/v1/live/start", token: discovery.token,
                                      body: ["operation_id": UUID().uuidString, "name": title])
            } else {
                if bridgeToken == nil { try await connectBridge() }
                _ = try await bridgeRequest("/v1/start", body: ["title": title])
            }
            await refresh()
        } catch { self.error = error.localizedDescription }
    }

    func control(_ action: String) async {
        error = nil
        do {
            if mode == .mac {
                guard let sessionID else { throw MenuError("Mac recorder has no active session") }
                let discovery = try readDiscovery()
                var body: [String: Any] = ["operation_id": UUID().uuidString, "session_id": sessionID]
                if let generation { body["expected_generation"] = generation }
                _ = try await request(discovery.baseURL + "/v1/live/\(action)", token: discovery.token, body: body)
            } else {
                _ = try await bridgeRequest("/v1/\(action)", body: [:])
            }
            await refresh()
        } catch { self.error = error.localizedDescription }
    }

    func connectBridge() async throws {
        guard !remote.trimmingCharacters(in: .whitespaces).isEmpty,
              !workspace.trimmingCharacters(in: .whitespaces).isEmpty else {
            throw MenuError("Enter a remote and Workspace")
        }
        guard let bridgeApp = ProcessInfo.processInfo.environment["MARGINS_MENU_BRIDGE_APP"],
              FileManager.default.fileExists(atPath: bridgeApp) else {
            throw MenuError("Set MARGINS_MENU_BRIDGE_APP to the scoped native bridge .app")
        }
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent("margins-menu-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: false,
                                                attributes: [.posixPermissions: 0o700])
        pairDirectory = directory
        let pairFile = directory.appendingPathComponent("pair-code")
        let audioDirectory = directory.appendingPathComponent("local-audio")
        var args = ["-n"]
        for (source, target) in [("MARGINS_MENU_SSH_REMOTE_BINARY", "MARGINS_SSH_REMOTE_BINARY"),
                                 ("MARGINS_MENU_SSH_REMOTE_DATA_DIR", "MARGINS_SSH_REMOTE_DATA_DIR"),
                                 ("MARGINS_MENU_HOME", "MARGINS_HOME")] {
            if let value = ProcessInfo.processInfo.environment[source] { args += ["--env", "\(target)=\(value)"] }
        }
        args += ["-a", bridgeApp, "--args", "native-bridge", "--remote", remote,
                 "--workspace", workspace, "--origin", bridgeOrigin, "--port", String(bridgePort),
                 "--pair-code-file", pairFile.path, "--local-audio-dir", audioDirectory.path]
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
        bridgePID = nil
        if let pairDirectory { try? FileManager.default.removeItem(at: pairDirectory) }
        pairDirectory = nil
        await refresh()
    }

    private func bridgeRequest(_ path: String, body: [String: Any]? = nil) async throws -> [String: Any] {
        guard let bridgeToken else { throw MenuError("Mac bridge is not paired") }
        return try await request("http://127.0.0.1:\(bridgePort)\(path)", token: bridgeToken,
                                 origin: bridgeOrigin, body: body)
    }

    private func readDiscovery() throws -> (baseURL: String, token: String) {
        let fallback = FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent("Library/Application Support/margins/desktop-live.v1.json").path
        let path = ProcessInfo.processInfo.environment["MARGINS_MENU_LOCAL_DISCOVERY"] ?? fallback
        let data = try Data(contentsOf: URL(fileURLWithPath: path))
        guard let value = try JSONSerialization.jsonObject(with: data) as? [String: Any],
              let baseURL = value["base_url"] as? String,
              let url = URL(string: baseURL), url.scheme == "http", url.host == "127.0.0.1",
              let token = value["token"] as? String else {
            throw MenuError("Invalid local recorder discovery file")
        }
        return (baseURL, token)
    }

    private func request(_ endpoint: String, token: String?, origin: String? = nil,
                         body: [String: Any]? = nil) async throws -> [String: Any] {
        guard let url = URL(string: endpoint) else { throw MenuError("Invalid recorder URL") }
        var request = URLRequest(url: url)
        request.timeoutInterval = 10
        if let token { request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization") }
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

@main
struct MarginsMenuApp: App {
    @StateObject private var recorder = MenuRecorder()

    var body: some Scene {
        MenuBarExtra("Margins", systemImage: "waveform") {
            VStack(alignment: .leading, spacing: 10) {
                Text("Margins recorder").font(.headline)
                Picker("Destination", selection: $recorder.mode) {
                    ForEach(CaptureMode.allCases) { mode in Text(mode.rawValue).tag(mode) }
                }.disabled(recorder.active)
                if recorder.mode == .project {
                    TextField("SSH alias or HTTPS URL", text: $recorder.remote)
                    TextField("Workspace ID", text: $recorder.workspace)
                }
                TextField("Meeting title", text: $recorder.title).disabled(recorder.active)
                Text(recorder.status).font(.caption).foregroundStyle(.secondary)
                if let error = recorder.error { Text(error).font(.caption).foregroundStyle(.red) }
                if let session = recorder.sessionID { Text("Session: \(session)").font(.caption2).textSelection(.enabled) }
                if !recorder.localAudioPaths.isEmpty {
                    Text("Mac audio copy: \(recorder.localAudioPaths.count) segment(s)").font(.caption2)
                }
                HStack {
                    Button("Start") { Task { await recorder.start() } }.disabled(recorder.active)
                    Button("Pause") { Task { await recorder.control("pause") } }.disabled(recorder.state != "recording")
                    Button("Resume") { Task { await recorder.control("resume") } }.disabled(recorder.state != "paused")
                    Button("Stop") { Task { await recorder.control("stop") } }.disabled(!recorder.active)
                }
                HStack {
                    Button("Refresh") { Task { await recorder.refresh() } }
                    if recorder.mode == .project {
                        Button("Disconnect") { Task { await recorder.disconnect() } }.disabled(recorder.active)
                    }
                    Button("Quit") {
                        Task {
                            await recorder.disconnect()
                            NSApp.terminate(nil)
                        }
                    }.disabled(recorder.active)
                }
            }
            .padding(14)
            .frame(width: 330)
        }
        .menuBarExtraStyle(.window)
    }
}
