# Deferred work

Work the owner approved but scheduled for later. Each item says why it waits,
where the code is ready for it, and when it is due. Nothing here is dropped;
an item leaves this list only when it ships or the owner removes it.

**Shipped and removed from this list:** autosave and crash recovery (see
[`design-autosave.md`](design-autosave.md)); saved shortcuts, the shortcut
editor and the saved dock layout (see "P-D part 2" in
[`design-project-panel.md`](design-project-panel.md)); precomp rendering and composition tabs (see "Milestone E" in
[`design-media-comps.md`](design-media-comps.md)).

| Item | Waits for | Code ready for it | Due |
|---|---|---|---|
| **Export overrides** (resolution and frame rate with scaling, without changing the composition) | The export dialog and encoders. | `RenderSettings` already separates output scale from the document, so an override never edits the composition. | Export milestone. |
| **Even-dimension check for encoders** | The encoders (Q5 in `design-proposal.md`). | Compositions may have odd sizes; the check belongs to export, which rounds or refuses per encoder. | Export milestone. |
| **Audio mixing from precomps** | Audio layers (no audio is mixed yet; Milestone 3b plays silence). | `Layer::child_time` is the single time mapping for video and audio; speed gives the resampling ratio. | Audio-layer milestone. |
| **Markers** | A marker model. | Drop snapping (Milestone D) takes a list of snap times, so markers are one more source. | After Milestone F. |
| **Footage blend / optical-flow resampling** | Later per-layer options. | Hold is the rule today (`design-media-comps.md` §8.2). | Unscheduled. |
| **"Hold child frame rate when nested"** | Later per-composition option. | Continuous sampling is the rule today (§8.2). | Unscheduled. |
| **Project panel thumbnails** (footage and compositions in the info header) | A preview renderer in the UI that can draw a composition other than the open one, cached by `(comp, revision, time)`. | `render_frame` takes any `CompId` (nested compositions render now), and the info header already reads the selected item; a painted icon stands in. | After tabs (next). |
| **Linked playheads, parent-time indicator and parent backdrop** (design §7.2, §7.3, §7.5) | Tabs, which now exist. Linked playheads need a per-tab toggle and an invertible time mapping; the indicator shades the child ruler outside the layer's mapped range. | `Layer::child_time` is the single mapping; tab views keep their own playheads; `NavStep::via_layer` says which layer a tab came in through. | After tabs (next). |
| **Viewport double-click and layer picking** | Hit testing in the viewport. | Double-click on a precomp layer works in the timeline; the viewport has no layer under the pointer yet, so Alt-replace and double-click there wait. | With viewport hit testing. |
