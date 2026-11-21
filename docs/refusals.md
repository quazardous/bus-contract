# Refusals

On a point declared `PointKind::Request`, a listener may refuse the
request with a reason (`HookError::refuse(reason)`). The request stops:
no later listener runs, the refusing listener's edit is dropped, and
the result names the refusal (`Altered::refused`: point, listener,
owner, reason). A refusal is not a failure and does not reach the
failure sinks. A refusal never rolls back what the listeners before
the refusing one did: if your listener has side effects, place it
after the listeners that may refuse (`after`, or a lower priority).

By default a failing listener (error, panic, missed deadline) has its
contribution dropped and the others run. On a point declared
`fail_closed`, the failure refuses the request instead: the failure is
reported as usual and `refused` is set with `failed: true`. A policy
listener placed first on a fail-closed point therefore never lets the
handler run when the policy itself breaks. If your policy listener
must hold even when it breaks, declare its point `fail_closed`.

```rust,ignore
pub const POINTS: &[PointDecl] =
    &[PointDecl::of::<CommandRun>("A command runs.").fail_closed()];

bus.listen::<CommandRun>("app.policy", ListenOpts::new().priority(1000), |run, _| {
    if run.origin.is_agent() && run.id == "files.delete" {
        return Err(HookError::refuse("agents may not delete files"));
    }
    Ok(())
})?;
let out = bus.alter::<CommandRun>(run, Outcome::default());
if let Some(refusal) = out.refused {
    // "hook `command.run`: refused by `app.policy`: agents may not delete files"
}
```
