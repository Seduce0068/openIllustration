import { createSignal, onMount } from "solid-js";
import init, { verify_checksum } from "../../integrity-verifier/pkg/integrity_verifier";

export default function ContentViewer(props: { cid: string; expected_checksum: string; reportId: number }) {
  const [verified, setVerified] = createSignal<boolean | null>(null);
  const [contentUrl, setContentUrl] = createSignal<string>("");

  onMount(async () => {
    await init();
    const res = await fetch(`http://localhost:3000/content/${props.cid}`);
    const blob = await res.blob();
    const arrayBuffer = await blob.arrayBuffer();
    const uint8Array = new Uint8Array(arrayBuffer);

    const isOk = verify_checksum(uint8Array, props.expected_checksum);
    setVerified(isOk);

    if (isOk) {
      setContentUrl(URL.createObjectURL(blob));
    }
  });

  const reportContent = async (reason: string) => {
    await fetch("http://localhost:3000/mod/report", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ target_cid: props.cid, reason }),
    });
    alert("Report submitted for jury review.");
  };

  const castVote = async (result: 'Approve' | 'Reject') => {
    await fetch("http://localhost:3000/mod/vote", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ report_id: props.reportId, juror_hash: "self", result }),
    });
    alert("Vote casted.");
  };

  return (
    <div class="p-4 border border-white/10 bg-black text-white">
      {verified() === true && <img src={contentUrl()} class="max-w-md mx-auto mb-4" />}
      <div class="flex gap-4 justify-center">
        <button onClick={() => reportContent("Inappropriate")} class="text-xs border px-2 py-1">Report</button>
        <button onClick={() => castVote('Approve')} class="text-xs border px-2 py-1 text-green-500">Approve</button>
        <button onClick={() => castVote('Reject')} class="text-xs border px-2 py-1 text-red-500">Reject</button>
      </div>
    </div>
  );
}
