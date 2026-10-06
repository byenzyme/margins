#!/usr/bin/env python3
"""Deterministic OpenAI-compatible catalyst generator for product tests."""

import argparse
import json
import re
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path


def catalysts_for(request):
    def message_text(message):
        content = message.get("content", "")
        if isinstance(content, str):
            return content
        if isinstance(content, list):
            return "\n".join(
                part.get("text", "") for part in content if isinstance(part, dict)
            )
        return ""

    prompt = "\n".join(
        message_text(message)
        for message in request.get("messages", [])
        if isinstance(message, dict)
    )
    match = re.search(r"Focus entity: ([^<\n]+)", prompt)
    entity = match.group(1).strip() if match else "the focus entity"
    anchor_id = "E1"
    exact_source_markers = [
        "Exact literal recall rescue",
        "SQLite runtime product order keeps this exact recall source visible",
    ]
    exact_marker = next((marker for marker in exact_source_markers if marker in prompt), None)
    if exact_marker:
        anchor_id = next(
            (
                candidate
                for candidate, _source_ref, evidence in re.findall(
                    r"\[(E\d+)\] source_ref=([^\s]+)[^\n]*\n(.*?)(?=\n\n\[E\d+\]|\n</vault_context>)",
                    prompt,
                    re.DOTALL,
                )
                if exact_marker in evidence
            ),
            anchor_id,
        )
    if entity.lower() == "bob@matching.test":
        anchor_id = next(
            (
                candidate
                for candidate, source_ref, evidence in re.findall(
                    r"\[(E\d+)\] source_ref=([^\s]+)[^\n]*\n(.*?)(?=\n\n\[E\d+\]|\n</vault_context>)",
                    prompt,
                    re.DOTALL,
                )
                if "t-shared" in source_ref
                or "DK shared the complete matching survey evidence" in evidence
                or "Thanks DK, I will review this with Bob and Josh" in evidence
                or (
                    "bob@matching.test" in evidence
                    and "broad mailbox item" not in evidence
                )
            ),
            anchor_id,
        )
    return {
        "catalysts": [
            f"What does {entity} need from the Tuesday pilot kickoff checkpoint?",
            f"How does {entity} shape the open product commitments in the evidence?",
            f"What changed for {entity} between planning, follow-up, and delivery?",
        ],
        "topicNames": ["pilot commitments", "kickoff decisions", "follow-up changes"],
        "eraLabels": ["fixture", "fixture", "fixture"],
        "anchorIds": [[anchor_id], [anchor_id], [anchor_id]],
    }


class Handler(BaseHTTPRequestHandler):
    count_file = None
    delay_seconds = 0.0
    request_count = 0
    count_lock = threading.Lock()

    def do_GET(self):
        if self.path == "/health":
            self._send({"ok": True})
        else:
            self.send_error(404)

    def do_POST(self):
        if self.path.rstrip("/") != "/v1/chat/completions":
            self.send_error(404)
            return
        length = int(self.headers.get("Content-Length", "0"))
        request = json.loads(self.rfile.read(length)) if length else {}
        with self.count_lock:
            type(self).request_count += 1
            if self.count_file is not None:
                self.count_file.write_text(f"{self.request_count}\n")
        if self.delay_seconds:
            time.sleep(self.delay_seconds)
        content = json.dumps(catalysts_for(request), separators=(",", ":"))
        self._send(
            {
                "id": "fixture-catalyst-completion",
                "object": "chat.completion",
                "model": "fixture-catalyst-model",
                "choices": [
                    {
                        "index": 0,
                        "message": {"role": "assistant", "content": content},
                        "finish_reason": "stop",
                    }
                ],
            }
        )

    def _send(self, payload):
        body = json.dumps(payload, separators=(",", ":")).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, _format, *_args):
        pass


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--port-file", type=Path, required=True)
    parser.add_argument("--count-file", type=Path)
    parser.add_argument(
        "--delay-ms", type=int, default=0, help="answer each completion after this delay"
    )
    args = parser.parse_args()
    Handler.count_file = args.count_file
    Handler.delay_seconds = args.delay_ms / 1000
    if Handler.count_file is not None:
        Handler.count_file.write_text("0\n")
    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    args.port_file.write_text(f"{server.server_port}\n")
    server.serve_forever()


if __name__ == "__main__":
    main()
