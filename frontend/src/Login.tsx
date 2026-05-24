import { createSignal } from "solid-js";

export default function Login() {
  const [nonce, setNonce] = createSignal("");
  const [loading, setLoading] = createSignal(false);

  const handleLogin = async () => {
    setLoading(true);
    try {
      const response = await fetch("http://localhost:3000/auth/login", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
      });
      const data = await response.json();
      setNonce(data.nonce);
      // In a production app, trigger OID4VP wallet here
      console.log("Authentication challenge received:", data.nonce);
    } catch (e) {
      console.error(e);
      alert("Failed to connect to the Guild Backend.");
    } finally {
      setLoading(false);
    }
  };

  return (
    <div class="flex flex-col items-center justify-center min-h-screen bg-black text-white font-mono p-4">
      <div class="border border-white/20 p-8 w-full max-w-md">
        <h1 class="text-2xl font-bold mb-6 tracking-widest text-center">GUILD ACCESS</h1>
        
        <button 
          onClick={handleLogin}
          disabled={loading()}
          class="w-full py-3 border border-white hover:bg-white hover:text-black transition-all duration-300 uppercase tracking-widest disabled:opacity-50"
        >
          {loading() ? "Initiating..." : "Initiate JPKI Auth"}
        </button>

        {nonce() && (
          <div class="mt-8 p-4 bg-white/5 border border-white/10">
            <p class="text-xs text-gray-400 mb-2">AUTH CHALLENGE</p>
            <p class="text-sm break-all">{nonce()}</p>
            <p class="mt-4 text-xs text-green-500 italic">Please present your JPKI credential via OID4VP wallet.</p>
          </div>
        )}
      </div>
    </div>
  );
}
