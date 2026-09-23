import AppKit
import SwiftUI

enum CaptureMode: String, CaseIterable, Identifiable {
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
    @Published var macTranscription = ""
    @Published var transcribingOnMac = false

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
        localAudioPaths = []
        sessionID = nil
        await refresh()
    }

    func transcribeMacCopy() async {
        guard !active, !localAudioPaths.isEmpty else { return }
        guard let binary = ProcessInfo.processInfo.environment["MARGINS_MENU_TRANSCRIBE_BIN"],
              let vault = ProcessInfo.processInfo.environment["MARGINS_MENU_TRANSCRIBE_VAULT"],
              let home = ProcessInfo.processInfo.environment["MARGINS_MENU_TRANSCRIBE_HOME"],
              binary.hasPrefix("/"), vault.hasPrefix("/"), home.hasPrefix("/") else {
            error = "Set absolute MARGINS_MENU_TRANSCRIBE_BIN, VAULT, and HOME paths"
            return
        }
        transcribingOnMac = true
        macTranscription = "Transcribing on this Mac…"
        error = nil
        do {
            try FileManager.default.createDirectory(atPath: vault, withIntermediateDirectories: true)
            try FileManager.default.createDirectory(atPath: home, withIntermediateDirectories: true)
            let paths = localAudioPaths
            for (index, path) in paths.enumerated() {
                try await Self.runLocalTranscription(binary: binary, vault: vault, home: home,
                                                     audioPath: path, segmentIndex: index)
            }
            macTranscription = "Mac CoreML transcript saved for \(paths.count) segment(s)"
        } catch {
            self.error = error.localizedDescription
            macTranscription = "Mac transcription needs attention"
        }
        transcribingOnMac = false
    }

    private nonisolated static func runLocalTranscription(binary: String, vault: String, home: String,
                                                           audioPath: String, segmentIndex: Int) async throws {
        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
            let process = Process()
            process.executableURL = URL(fileURLWithPath: binary)
            process.currentDirectoryURL = URL(fileURLWithPath: vault, isDirectory: true)
            process.arguments = ["--local", "transcribe", audioPath, "--name", "menu-\(UUID().uuidString)-seg\(segmentIndex)"]
            var environment = ProcessInfo.processInfo.environment
            environment["MARGINS_HOME"] = home
            environment["MARGINS_PROFILE"] = "menu-test"
            environment.removeValue(forKey: "MARGINS_REMOTE")
            process.environment = environment
            let log = URL(fileURLWithPath: vault).appendingPathComponent("menu-transcribe-\(UUID().uuidString).log")
            FileManager.default.createFile(atPath: log.path, contents: nil,
                                           attributes: [.posixPermissions: 0o600])
            guard let output = FileHandle(forWritingAtPath: log.path) else {
                continuation.resume(throwing: MenuError("Could not open Mac transcription log"))
                return
            }
            process.standardOutput = output
            process.standardError = output
            process.terminationHandler = { finished in
                try? output.close()
                if finished.terminationStatus == 0 { continuation.resume(returning: ()) }
                else { continuation.resume(throwing: MenuError("Mac transcription failed; inspect \(log.path)")) }
            }
            do { try process.run() }
            catch {
                try? output.close()
                continuation.resume(throwing: error)
            }
        }
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
                    Button("Transcribe Mac copy") { Task { await recorder.transcribeMacCopy() } }
                        .disabled(recorder.active || recorder.transcribingOnMac)
                    if !recorder.macTranscription.isEmpty {
                        Text(recorder.macTranscription).font(.caption2)
                    }
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
                        Button("Disconnect") { Task { await recorder.disconnect() } }
                            .disabled(recorder.active || recorder.transcribingOnMac)
                    }
                    Button("Quit") {
                        Task {
                            await recorder.disconnect()
                            NSApp.terminate(nil)
                        }
                    }.disabled(recorder.active || recorder.transcribingOnMac)
                }
            }
            .padding(14)
            .frame(width: 330)
        }
        .menuBarExtraStyle(.window)
    }
}
