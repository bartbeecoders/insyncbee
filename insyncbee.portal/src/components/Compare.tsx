type Cell = boolean | string;
interface Row {
  feature: string;
  drive: Cell;
  insync: Cell;
  rclone: Cell;
  us: Cell;
}

const ROWS: Row[] = [
  { feature: "Native Linux support",       drive: false,            insync: true,       rclone: true,          us: true },
  { feature: "Native GUI",                 drive: true,             insync: true,       rclone: false,         us: true },
  { feature: "Block-level delta sync",     drive: false,            insync: false,      rclone: false,         us: true },
  { feature: "On-demand placeholders",     drive: "Buggy",          insync: false,      rclone: "Via mount",   us: true },
  { feature: "Real-time sync",             drive: true,             insync: true,       rclone: false,         us: true },
  { feature: "Conflict preview & resolve", drive: false,            insync: "Basic",    rclone: false,         us: true },
  { feature: "Bandwidth control",          drive: "Admin only",     insync: false,      rclone: true,          us: true },
  { feature: "Open source",                drive: false,            insync: false,      rclone: true,          us: true },
  { feature: "Price",                      drive: "Free",           insync: "$40/acct", rclone: "Free",        us: "Free" },
];

function Cell({ value }: { value: Cell }) {
  if (value === true) return <span className="cell-yes">● Yes</span>;
  if (value === false) return <span className="cell-no">○ No</span>;
  return <span className="text-muted">{value}</span>;
}

export default function Compare() {
  return (
    <section id="compare">
      <div className="container">
        <span className="eyebrow">How we compare</span>
        <h2>The only sync client built for data safety first.</h2>
        <p className="section-intro">
          Google's desktop client skips Linux. Insync costs per-account and leaks
          memory. rclone has no GUI. InSyncBee fills the gap.
        </p>

        <div className="compare-table-wrap">
          <table className="compare-table">
            <thead>
              <tr>
                <th>Feature</th>
                <th>Google Drive</th>
                <th>Insync</th>
                <th>rclone</th>
                <th className="col-us">InSyncBee</th>
              </tr>
            </thead>
            <tbody>
              {ROWS.map((row) => (
                <tr key={row.feature}>
                  <td>{row.feature}</td>
                  <td><Cell value={row.drive} /></td>
                  <td><Cell value={row.insync} /></td>
                  <td><Cell value={row.rclone} /></td>
                  <td className="cell-us"><Cell value={row.us} /></td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </div>
    </section>
  );
}
