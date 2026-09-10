# Evaluating agent and prompt behavior

A prompt evaluation is an experiment about behavior. It is not a contest between
two impressive outputs, and it is not proof that a larger prompt understands more.

The work begins by naming the judgment we want the system to make differently. Then
we change as little as possible, preserve what the model actually saw, and invite an
independent reader to prove our story wrong.

## Start with the behavior

Write the question in human terms before writing an arm or a rubric.

Good questions:

- Does the assistant read a private memo as the user's attention rather than as a
  claim about the other person?
- Does relevant history improve interpretation without creating pressure to speak?
- Can the assistant tell the difference between a weak idea and an opening that has
  already passed?
- Does a prior trajectory help preparation without displacing concrete facts?

Weak questions:

- Is prompt B better?
- Does more context help?
- Does the new persona feel smarter?

A behavioral question tells us what to hold constant, what to vary, and what failure
would look like.

## Separate the layers

Agent behavior usually comes from several constructions that are easy to confuse:

1. **Posture** — who the model is trying to be and what it attends to.
2. **Evidence** — transcript, history, retrieved sources, tools, and private context.
3. **Decision policy** — whether to act, wait, ask, abstain, or declare uncertainty.
4. **Rendering** — the shape and voice of the visible response.
5. **Protocol** — schemas, labels, telemetry, retries, and orchestration.

Changing all five at once may produce a better system, but it does not teach us which
change mattered. Call that a system comparison, not a prompt ablation.

This distinction is especially important for personas. A persona is an attentional
posture, not a state machine. “A present listener with continuity in peripheral
vision” is a persona. `trigger_class`, `history_used`, and fourteen cue types are
protocol.

## State the contrast before spending

For every arm, write down:

| Arm | Held constant | Changed | Claim this comparison may support |
| --- | --- | --- | --- |
| A | Exact shared inputs | Nothing; baseline | Reference behavior |
| B | Model, evidence, schema, sampling plan | Persona paragraph only | Effect of posture |
| C | Model, persona, schema, sampling plan | One context block | Effect of that context |

If the “changed” cell contains several things, either split the experiment or narrow
the claim.

Useful contrasts include:

- production prompt vs production plus one posture paragraph;
- no history vs one deliberately selected relevant connection;
- relevant history vs irrelevant history as a pollution control;
- the same evidence with and without a prior trajectory;
- act vs abstain at the same moment;
- one system construction vs another, explicitly labeled observational.

Do not use an old run as the only comparator when the model, prompt, retrieval, or
code path has since changed. Run contemporaneous controls.

## Freeze and preserve the real prompts

Before generation:

- freeze prompt sources;
- persist every assembled system and user prompt;
- record model and provider identity as honestly as the runtime exposes it;
- hash or byte-compare prompts for arms claimed to be identical;
- record source manifests, cutoffs, and context lengths;
- state the sample count and retry policy.

A source-code diff is not proof that assembled prompts differ only where intended.
Runtime settings, default instructions, retrieval order, and session identifiers can
enter the prompt unexpectedly.

Use a golden test when production behavior is meant to remain unchanged.

## Treat context as an intervention

Context is not passive. Its presence changes the model's sense of obligation.
Relevant-looking history can cause a system to find a pattern, mention a fact, or
intervene merely because the material is available.

Test context with the same care as instructions:

- no context;
- relevant context;
- irrelevant but plausible context;
- contradictory context;
- context loaded but explicitly optional.

If packet and retrieval blocks change together, the result belongs to the combined
history condition. It cannot be assigned to the packet alone.

Upstream evidence quality is part of the experiment. Identity resolution, temporal
cutoffs, redaction, and source eligibility must be verified before prompting. A model
cannot recover from two people being presented as one.

For relationship history, separate three gates that are easy to collapse:

1. **Entity recall** — did the named person's catalyst/profile participate?
2. **Neighborhood integrity** — do candidate documents actually contain that entity,
   rather than merely resemble the query?
3. **Moment materiality** — does one prior commitment, change, contradiction, or
   unresolved tension alter what would be useful now?

A relational catalyst can solve the first gate while still returning globally similar
notes about other work. Constrain candidates with the first-class entity index before
asking a model to judge materiality. The materiality decision must be nullable: a rich
person arc is permission to consider history, not permission to place it in the prompt.

## Make abstention a first-class result

For assistants that interrupt, message, recommend, or act, the negative space is
part of the product.

Include fixtures where the right behavior is:

- stay quiet;
- wait for a better opening;
- admit there is not enough evidence;
- preserve an unresolved fork;
- decline to use stale or off-topic history.

Evaluate false-positive action separately from response quality. A beautiful answer
shown at the wrong moment is a failure.

Availability of evidence must never count as evidence that the agent should act.

## Let evidence choose the artifact's shape

For synthesis and preparation work, a visible outline is also a decision policy. A
required `Trajectory` or `Throughline` section pressures the model to manufacture that
kind of meaning even when the source material only supports adjacent fragments, a
contrast, or an open fork.

Prefer a posture that decides privately how much coherence the evidence earns. Keep
the visible artifact free to take the shortest useful form, and keep audit structure in
a generic provenance ledger rather than making provenance categories double as the
reader's outline.

Freedom of form needs its own attention discipline. Ask for the shortest artifact that
preserves what the user should notice or do, put concrete actions from the freshest
sources before broader interpretation, and end with something the user can carry into
the next situation. Evaluate usefulness per unit of attention, not length alone: a
slightly longer artifact can win when every extra line buys a consequential distinction
or move, while a beautiful expansive essay can still fail as preparation.

When sources prescribe an action, test whether the synthesis actually promotes it. A
generator can correctly reconstruct the surrounding narrative and still miss the most
useful instruction in the freshest note. Likewise, when a source names an earlier date
or session, prefer the original source; if only a later recap supports the claim, make
that mediation visible.

## Keep instrumentation from becoming the prompt

Observability fields can change behavior. Asking a model to name the history it used
may make it search for history to justify the field. Asking it to classify a memo may
collapse an ambiguous reading into the available labels.

Prefer external evidence:

- prompt and tool traces;
- source-span attribution;
- history/no-history ablations;
- deterministic call counts;
- model outputs preserved before parsing or validation.

Treat model self-report as a hypothesis, not ground truth. If telemetry is necessary,
keep it private, allow honest emptiness, and test whether adding the field changes the
visible behavior.

Persist raw completions immediately after the model returns. Validation should fail
closed without destroying the evidence needed to understand the failure.

## Judge blind, then reveal

Use two passes.

### Phase 1: behavior

Show neutral, shuffled candidates. Hide arm names, prompts, retrieval provenance,
telemetry, and the desired conclusion. Ask the judge what is useful, harmful, late,
generic, invasive, or better left unsaid.

Record the judgment verbatim before reveal.

### Phase 2: causality

Reveal the conditions, exact prompt differences, context, provenance, and known
confounds. Ask what most likely caused the observed difference and what the evidence
cannot distinguish.

The same judge can diagnose after reveal, but preserve the blind answer first. When
taste or risk matters, use an independent judge rather than the prompt author.

## Respect sampling variance

One generation can disprove a brittle claim, but it rarely establishes a subtle
preference.

- Repeat the cells carrying the main causal claim.
- Predeclare repeats before seeing which arm wins.
- Compare within-condition variance with between-condition differences.
- Treat close scores as ties when sampling variance is of similar size.
- Preserve every attempt, including malformed and degraded calls.

Large, repeated, same-direction failures—such as acting twice when the control stays
quiet—deserve attention even in a small study. Small wording wins do not.

## Revise posture before adding machinery

When a persona fails, resist adding labels and rules until the attentional mistake is
clear.

Prefer a few ordered commitments:

> Attend to the present first. Treat private context as peripheral vision. History
> may sharpen understanding, but it never creates an obligation to speak. Preserve
> uncertainty. If the opening has passed, stay quiet.

This is easier to evaluate than a persona entangled with taxonomies, telemetry, and
branching states.

Keep personas for genuinely different jobs separate. A skeptical editor preparing
someone for a meeting and a quiet listener sitting inside that meeting should not be
forced into one voice or one prompt.

Then test whether posture is actually the controlling layer. A concise, coherent
persona can be behaviorally inert when the shared user prompt, response schema, or
renderer gives the model a stronger instruction. If every request says that the user
explicitly asked for a suggestion and every valid response contains a suggestion,
an appended “stay quiet” posture is not a fair test of abstention.

The action space must contain the behavior being evaluated. Before testing quiet,
uncertainty, or deferment, verify that the unchanged protocol can represent it and
that no higher-priority instruction contradicts it.

## Test when context acts, not only whether it matches

A connection can be relevant to the relationship and wrong for the present moment.
Topical relevance is therefore not enough for a live assistant.

Include at least three opportunities in a context test:

- a moment where the connection should materially sharpen the question;
- a nearby moment where the connection is true but should remain peripheral;
- an attentional vacuum where the assistant should not turn available history into
  an agenda.

This distinguishes useful recall from mistimed importation. A model that quotes the
right history at the wrong moment has passed retrieval and failed judgment.

When the same context is repeatedly mistimed across otherwise identical prompts,
stop tuning retrieval rank. Gate context before generation or change the permission
structure of the request.

## Audit fixtures semantically, not just temporally

A source dated before a replay can still summarize or quote the replayed session.
Dates, hashes, and filename cutoffs will not catch that causal leak.

Before spending:

- establish the source session and target session independently;
- search the fixture for target memos, steering phrases, and distinctive transcript
  language;
- explain in one sentence what new distinction the fixture could contribute at a
  named mark;
- quarantine the complete arm if provenance fails after generation—do not repair the
  interpretation by selectively keeping favorable outputs.

Preserving invalid runs is useful accounting. It does not make them evidence.

## Report in the order a reader thinks

The main report should be readable without the runbook.

1. The question.
2. What was held constant and what changed.
3. The finding.
4. The strongest counterexample.
5. What the experiment could not prove.
6. The smallest justified revision.
7. The next experiment.

Put call ledgers, hashes, command lines, integrity incidents, and full judge outputs
in an appendix. They make the result trustworthy; they are not the story.

Use precise claim labels:

- **Causal:** the relevant input changed while the rest was verified identical.
- **Directional:** evidence points one way but sampling or scope is small.
- **Observational:** whole constructions differed.
- **Confounded:** another difference could plausibly explain the result.
- **Not tested:** attractive inference, absent contrast.

## A compact preflight

Before running a prompt-behavior evaluation, confirm:

- [ ] The behavioral question and falsifier are written plainly.
- [ ] Persona, context, policy, rendering, and protocol changes are separated.
- [ ] Every arm states what is constant and what changes.
- [ ] The strongest claim has a contemporaneous control.
- [ ] Assembled prompts and source manifests will be preserved.
- [ ] Temporal and identity integrity are checked upstream.
- [ ] Abstention and pollution fixtures are included.
- [ ] The unchanged output space can represent abstention or uncertainty.
- [ ] Relevant context has one named opportunity to help and one opportunity where
      it must stay peripheral.
- [ ] Context provenance is semantically independent of the target, not merely older.
- [ ] Raw completions survive parser or validator failure.
- [ ] Sample counts, retries, and stop rules are predeclared.
- [ ] Blind judgment precedes causal reveal.
- [ ] The report will lead with findings and limitations, not mechanics.

The point is not to make prompting scientific in appearance. It is to make our
learning honest enough that the next revision is smaller, clearer, and more likely to
work.
