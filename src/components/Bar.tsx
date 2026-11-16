import { clamp } from "../lib/format";

type Props = {
  label: string;
  value: number;
  /** 右侧展示文本，例如 8.2/16.0 GiB */
  detail: string;
  color: string;
};

/** 细横条：label + 进度 + 数值 */
export function Bar({ label, value, detail, color }: Props) {
  const pctValue = clamp(value, 0, 100);
  return (
    <div className="bar">
      <div className="bar-head">
        <span style={{ color }}>{label}</span>
        <span className="mono bar-value">{detail}</span>
      </div>
      <div className="bar-track">
        <div
          className="bar-fill"
          style={{ width: `${pctValue}%`, background: color, transition: "width 400ms ease" }}
        />
      </div>
    </div>
  );
}