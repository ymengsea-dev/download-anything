import type { Kind } from "../../shared/api";

export function SectionHead({ kind, label, count }: { kind: Kind | "render"; label: string; count?: number }) {
  return (
    <div className={`section-head ${kind}`}>
      <span className="dot" />
      <span className="section-label">{label}</span>
      {count !== undefined && count > 1 && <span className="count">{count}</span>}
    </div>
  );
}
