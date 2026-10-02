# Grow a personal CRM from your notes

A CRM, for one person, is three things: the people you deal with, a record of every time you dealt with them, and a nudge about what you owe them next. Strip away the sales-team machinery and that's notes, the people in them, and links that pile up over time.

Margins already gives you two of the three.

**What ships today.** Every distilled note names its participants in the frontmatter and links them in the body:

```yaml
---
people: ["[[Alice Chen]]", "[[Ben Ortiz]]"]
---
```

Because every note lands in the same folder, your notes app shows you, for free, every meeting that mentions a person — open `[[Alice Chen]]` and read the backlinks. That's the interaction history. You didn't log it; recording the meeting logged it.

**What you build.** The two CRM parts Margins leaves to you:

- **A profile per person.** Make an `Alice Chen.md`. Ask an agent to read every note that links her and roll it up: how you met, recurring themes, what she's working on, the last time you talked.
- **What you owe.** Margins puts action items in each note. Ask an agent to sweep the folder for open checkboxes tied to a person and list them on that person's page.

A prompt to start from:

> Read every note in this folder that links `[[Alice Chen]]`. Write or update `Alice Chen.md` with: how we know each other, the topics that keep coming up, open action items where either of us owes the other something, and the date we last spoke. Link back to each note you drew from.

Run it after each meeting, or on a schedule. The folder is plain Markdown and local — the agent reads what's already there, and the CRM is a view you regenerate, never a database you maintain.

Margins feeds this. It doesn't own it. Change what a person page is by changing the prompt.
