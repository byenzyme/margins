---
name: margins-watermark
description: Give a quick, transcript-grounded read during a Margins meeting when the user asks for a watermark, what to say next, a sanity check, or how to steer the live conversation.
---

# Live meeting watermark

Call the Margins plugin's `watermark_snapshot` anew for every request. If this chat already pinned a session ID, pass it; otherwise let the tool resolve the active recording. When several active IDs are returned, ask which meeting the user means. Do not silently switch sessions within one chat.

Use the returned complete transcript, weighing the newest complete turn most heavily. `decoded_until_ms` is the watermark. A missing transcript means there are no usable words yet; say so instead of inventing a read. Treat nonterminal Mac CoreML words as provisional, and make uncertainty clear when speech or speaker attribution is unclear.

Answer the user's actual question first, in a form they can use during the conversation. A useful default is a short read, one sentence they could say next, and one thing to watch. Keep the first answer concise. Do not run recall or search old notes before that answer; this workflow is time sensitive. On a later request, fetch a new snapshot and update the read rather than relying on the previous response.

When capture has ended, say the meeting is finished and use the final transcript if available. Full retrospective synthesis belongs to `margins-distill`.
