import { EditorView } from "@codemirror/view";

type Position = {anchor:number; offset:number};
/** Per-library/document display state; no note text is stored here. */
export class ReadingPositions {
  private key?: string;
  private restoring = false;
  private epoch = 0;
  private timer?: ReturnType<typeof setTimeout>;
  constructor(private view: EditorView) {
    view.scrollDOM.addEventListener("scroll", () => {
      clearTimeout(this.timer);
      this.timer = setTimeout(() => this.save(), 120);
    }, {passive:true});
    window.addEventListener("pagehide", () => this.save());
    window.addEventListener("beforeunload", () => this.save());
  }
  save(): void {
    if (!this.key || this.restoring || !this.view.scrollDOM.clientHeight) return;
    const view = this.view;
    try {
      const block = view.lineBlockAtHeight(view.scrollDOM.scrollTop);
      const offset = view.documentTop + block.top - view.scrollDOM.getBoundingClientRect().top;
      localStorage.setItem(this.key, JSON.stringify({anchor:block.from, offset}));
    } catch { /* Unavailable browser storage never prevents opening a note. */ }
  }
  leave(): void {
    this.save(); clearTimeout(this.timer); this.key = undefined;
    this.restoring = false; this.epoch++;
  }
  restore(identity: unknown): void {
    if (typeof identity !== "string" || !identity || identity.length > 8192) return;
    this.key = "studyapp.reading.v1:" + identity;
    let saved: Position;
    try {
      saved = JSON.parse(localStorage.getItem(this.key) || "null");
      if (!saved || !Number.isSafeInteger(saved.anchor) || saved.anchor < 0 || !Number.isFinite(saved.offset) || Math.abs(saved.offset) > 100000) return;
    } catch { return; }
    this.restoring = true;
    const epoch = ++this.epoch, view = this.view;
    const anchor = Math.min(saved.anchor, view.state.doc.length);
    view.dispatch({effects:EditorView.scrollIntoView(anchor, {y:"start", yMargin:0})});
    requestAnimationFrame(() => requestAnimationFrame(() => {
      if (epoch !== this.epoch) return;
      view.requestMeasure({
        read: () => view.documentTop + view.lineBlockAt(anchor).top - view.scrollDOM.getBoundingClientRect().top,
        write: top => {
          if (epoch !== this.epoch) return;
          view.scrollDOM.scrollTop += top - saved.offset;
          this.restoring = false;
        },
      });
    }));
  }
}
