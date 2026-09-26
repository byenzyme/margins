import Foundation
import Network

struct MenuGrant: Decodable {
    let serviceUrl: String
    let token: String
    let workspaceId: String
    let workspaceName: String
    let instanceId: String
    let expiresAt: Int64
}

/// A small loopback entry point for the Connect button in bb. The grant is
/// verified by the bb server before it can configure or start the recorder.
final class MenuPairingServer {
    private let queue = DispatchQueue(label: "margins.menu.pairing")
    private let listener: NWListener
    private weak var recorder: MenuRecorder?
    private let port: UInt16 = 18764

    init(recorder: MenuRecorder) throws {
        self.recorder = recorder
        let parameters = NWParameters.tcp
        parameters.requiredLocalEndpoint = .hostPort(host: "127.0.0.1", port: NWEndpoint.Port(rawValue: port)!)
        listener = try NWListener(using: parameters)
        listener.newConnectionHandler = { [weak self] connection in
            guard let self else { connection.cancel(); return }
            connection.start(queue: self.queue)
            self.receive(connection, accumulated: Data())
        }
        listener.start(queue: queue)
    }

    private func receive(_ connection: NWConnection, accumulated: Data) {
        connection.receive(minimumIncompleteLength: 1, maximumLength: 65_536) { [weak self] chunk, _, complete, error in
            guard let self else { connection.cancel(); return }
            var bytes = accumulated
            if let chunk { bytes.append(chunk) }
            if bytes.count > 2_200_000 || error != nil { self.respond(connection, status: 400, body: ["error": "invalid_request"], origin: nil); return }
            guard let boundary = bytes.range(of: Data("\r\n\r\n".utf8)) else {
                if complete || bytes.count > 16_384 { self.respond(connection, status: 400, body: ["error": "invalid_request"], origin: nil) }
                else { self.receive(connection, accumulated: bytes) }
                return
            }
            guard boundary.lowerBound <= 16_384 else {
                self.respond(connection, status: 400, body: ["error": "invalid_headers"], origin: nil); return
            }
            let headerBytes = bytes[..<boundary.lowerBound]
            guard let headerText = String(data: headerBytes, encoding: .utf8) else {
                self.respond(connection, status: 400, body: ["error": "invalid_headers"], origin: nil); return
            }
            let lines = headerText.components(separatedBy: "\r\n")
            let request = lines.first?.split(separator: " ").map(String.init) ?? []
            var headers: [String: String] = [:]
            for line in lines.dropFirst() {
                guard let separator = line.firstIndex(of: ":") else { continue }
                let key = line[..<separator].lowercased()
                if headers[key] != nil { self.respond(connection, status: 400, body: ["error": "duplicate_header"], origin: nil); return }
                headers[key] = line[line.index(after: separator)...].trimmingCharacters(in: .whitespaces)
            }
            guard request.count == 3, headers["host"] == "127.0.0.1:\(port)" else {
                self.respond(connection, status: 403, body: ["error": "origin_rejected"], origin: nil); return
            }
            let relayPath = "/api/v1/plugins/margins/http/menu/relay"
            let isRelay = request[0] == "POST" && request[1] == relayPath
            if isRelay && headers["origin"] != nil {
                self.respond(connection, status: 403, body: ["error": "origin_rejected"], origin: nil); return
            }
            guard isRelay || headers["origin"].map(self.allowed) == true else {
                self.respond(connection, status: 403, body: ["error": "origin_rejected"], origin: nil); return
            }
            let origin = headers["origin"]
            let length = Int(headers["content-length"] ?? "0") ?? -1
            guard length >= 0 && length <= (isRelay ? 2_100_000 : 8_192) else {
                self.respond(connection, status: 413, body: ["error": "payload_too_large"], origin: origin); return
            }
            let body = bytes[boundary.upperBound...]
            guard body.count >= length else {
                if complete { self.respond(connection, status: 400, body: ["error": "incomplete_body"], origin: origin) }
                else { self.receive(connection, accumulated: bytes) }
                return
            }
            if isRelay {
                guard let auth = headers["authorization"], auth.hasPrefix("Bearer ") else {
                    self.respond(connection, status: 403, body: ["error": "grant_required"], origin: nil); return
                }
                self.handleRelay(connection, body: Data(body.prefix(length)), token: String(auth.dropFirst(7)))
            } else if let origin {
                self.handle(connection, method: request[0], path: request[1], origin: origin, body: Data(body.prefix(length)))
            }
        }
    }

    private func handleRelay(_ connection: NWConnection, body: Data, token: String) {
        Task { @MainActor in
            guard let recorder = self.recorder else {
                self.respond(connection, status: 503, body: ["error": "menu_unavailable"], origin: nil); return
            }
            do {
                let result = try await recorder.forwardCaptureRelay(body, token: token)
                self.respond(connection, status: 200, body: result, origin: nil)
            } catch {
                self.respond(connection, status: 502, body: ["error": "capture_relay_unavailable"], origin: nil)
            }
        }
    }

    private func allowed(_ origin: String) -> Bool {
        guard let url = URL(string: origin), url.absoluteString == origin, url.path.isEmpty || url.path == "/",
              url.query == nil, url.fragment == nil, let host = url.host else { return false }
        if url.scheme == "https" { return host == "getbb.app" || host.hasSuffix(".getbb.app") }
        return url.scheme == "http" && ["127.0.0.1", "localhost"].contains(host)
    }

    private func handle(_ connection: NWConnection, method: String, path: String, origin: String, body: Data) {
        if method == "OPTIONS" {
            respond(connection, status: 204, body: [:], origin: origin)
            return
        }
        Task { @MainActor in
            guard let recorder = self.recorder else { self.respond(connection, status: 503, body: ["error": "menu_unavailable"], origin: origin); return }
            if let pinned = recorder.connectedOrigin, pinned != origin {
                self.respond(connection, status: 409, body: ["error": "different_bb_origin_connected"], origin: origin); return
            }
            if method == "GET" && path == "/v1/probe" {
                self.respond(connection, status: 200, body: ["available": true,
                    "workspaceId": recorder.connectedOrigin == nil ? NSNull() : recorder.workspace], origin: origin)
                return
            }
            if method == "POST" && path == "/v1/connect" {
                do {
                    let grant = try JSONDecoder().decode(MenuGrant.self, from: body)
                    let paired = try await recorder.connectGrant(grant, origin: origin)
                    self.respond(connection, status: 200, body: paired, origin: origin)
                } catch {
                    self.respond(connection, status: 400, body: ["error": error.localizedDescription], origin: origin)
                }
                return
            }
            self.respond(connection, status: 404, body: ["error": "not_found"], origin: origin)
        }
    }

    private func respond(_ connection: NWConnection, status: Int, body: [String: Any], origin: String?) {
        let payload = status == 204 ? Data() : ((try? JSONSerialization.data(withJSONObject: body)) ?? Data("{}".utf8))
        let reason = [200: "OK", 204: "No Content", 400: "Bad Request", 403: "Forbidden", 404: "Not Found", 409: "Conflict", 413: "Payload Too Large", 502: "Bad Gateway", 503: "Service Unavailable"][status] ?? "Error"
        var headers = "HTTP/1.1 \(status) \(reason)\r\nContent-Type: application/json\r\nContent-Length: \(payload.count)\r\nCache-Control: no-store\r\nConnection: close\r\n"
        if let origin {
            headers += "Access-Control-Allow-Origin: \(origin)\r\nVary: Origin\r\nAccess-Control-Allow-Methods: GET, POST, OPTIONS\r\nAccess-Control-Allow-Headers: content-type\r\nAccess-Control-Allow-Private-Network: true\r\n"
        }
        headers += "\r\n"
        var response = Data(headers.utf8)
        response.append(payload)
        connection.send(content: response, completion: .contentProcessed { _ in connection.cancel() })
    }
}
