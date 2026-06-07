<!-- event: create author: "yoi ticket" at: 2026-06-06T23:35:20Z -->

## Created

Created by LocalTicketBackend create.

---

<!-- event: plan author: hare at: 2026-06-06T23:36:10Z -->

## Plan

Created from investigation of perceived delays when opening `yoi panel`, attaching to a Pod, and returning from an attached Pod.

Root cause recorded:
- the panel waits for full snapshot/Orchestrator work before first draw;
- attach waits for socket connect or restore/spawn before a visible transition;
- return from nested Pod awaits `app.reload_or_notice().await` before showing the panel again.

Desired direction:
- show the panel/progress state first;
- move reload/connect/ensure work into background or explicit progress states where practical;
- especially make return-from-attach redraw the previous panel immediately and refresh in the background.


---
