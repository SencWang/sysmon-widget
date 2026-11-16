type Series = { values: number[]; color: string; label: string };

type Props = {
  title: string;
  series: Series[];
  /** 期望点数，用于右侧对齐 */
  points?: number;
  right?: string;
};

function polyline(values: number[], total: number): string {
  return values
    .map((v, i) => {
      const offset = Math.max(0, total - values.length);
      const x = (600 * (offset + i)) / Math.max(1, total - 1);
      const y = 100 - Math.min(100, Math.max(0, v));
      return `${x.toFixed(2)},${y.toFixed(2)}`;
    })
    .join(" ");
}

/** 历史折线：Y 轴 0-100 网格 + 刻度，自写 SVG（无图表库） */
export function LineChart({ title, series, points = 180, right }: Props) {
  return (
    <section className="chart-card">
      <header className="chart-head">
        <span className="chart-title">{title}</span>
        <span className="legend">
          {series.map((s) => (
            <span key={s.label} className="legend-item">
              <i style={{ background: s.color }} />
              {s.label}
            </span>
          ))}
        </span>
        {right ? <span className="mono dim chart-right">{right}</span> : null}
      </header>
      <div className="chart-body">
        <div className="chart-axis mono">
          <span>100</span>
          <span>75</span>
          <span>50</span>
          <span>25</span>
          <span>0</span>
        </div>
        <svg className="chart-svg" viewBox="0 0 600 100" preserveAspectRatio="none">
          {[0, 25, 50, 75, 100].map((v) => (
            <line
              key={v}
              x1="0"
              x2="600"
              y1={100 - v}
              y2={100 - v}
              stroke={v === 0 || v === 100 ? "rgba(255,255,255,0.18)" : "rgba(255,255,255,0.08)"}
              strokeWidth="1"
              vectorEffect="non-scaling-stroke"
            />
          ))}
          {series.map((s) => (
            <polyline
              key={s.label}
              points={polyline(s.values, points)}
              fill="none"
              stroke={s.color}
              strokeWidth="1.6"
              strokeLinejoin="round"
              strokeLinecap="round"
              vectorEffect="non-scaling-stroke"
            />
          ))}
        </svg>
      </div>
      <footer className="chart-foot mono dim">
        <span>60 秒前</span>
        <span>现在</span>
      </footer>
    </section>
  );
}