# Deferred work

Work the owner approved but scheduled for later. Each item says why it waits,
where the code is ready for it, and when it is due. Nothing here is dropped;
an item leaves this list only when it ships or the owner removes it.

| Item | Waits for | Code ready for it | Due |
|---|---|---|---|
| **Autosave and crash recovery** | Its own milestone. | `persist::save_with_ui` writes the document and `ui_state` together, atomically; recovery files use the same envelope. | **After Milestone F.** Must not be dropped. |
| **Export overrides** (resolution and frame rate with scaling, without changing the composition) | The export dialog and encoders. | `RenderSettings` already separates output scale from the document, so an override never edits the composition. | Export milestone. |
| **Even-dimension check for encoders** | The encoders (Q5 in `design-proposal.md`). | Compositions may have odd sizes; the check belongs to export, which rounds or refuses per encoder. | Export milestone. |
| **Audio mixing from precomps** | Audio layers (no audio is mixed yet; Milestone 3b plays silence). | `Layer::child_time` is the single time mapping for video and audio; speed gives the resampling ratio. | Audio-layer milestone. |
| **Markers** | A marker model. | Drop snapping (Milestone D) takes a list of snap times, so markers are one more source. | After Milestone F. |
| **Precomp rendering** | Milestone E. | `evaluate` already yields `EvaluatedContent::Precomp { comp, child_time, size }`; `RenderPlan::build` draws nothing for it until then. | Milestone E. |
| **Footage blend / optical-flow resampling** | Later per-layer options. | Hold is the rule today (`design-media-comps.md` §8.2). | Unscheduled. |
| **"Hold child frame rate when nested"** | Later per-composition option. | Continuous sampling is the rule today (§8.2). | Unscheduled. |
