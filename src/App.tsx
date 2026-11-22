import { useState } from "react";
import { AppProvider, useApp } from "./app/AppContext";
import { feature } from "./app/features";
import { Shell } from "./app/Shell";
import { WhatsNew } from "./components/Discover";
import { ConnectionRequestDialog, Splash, Toasts } from "./components/Overlays";
import { MixerPage } from "./pages/mixer/MixerPage";
import { ComingSoonPage } from "./pages/ComingSoonPage";
import { InputPage } from "./pages/InputPage";
import { HomePage } from "./pages/HomePage";
import { SettingsPage } from "./pages/SettingsPage";

function CurrentPage() {
  const { route } = useApp();
  switch (route.page) {
    case "audio":
      return <MixerPage />;
    case "settings":
      return <SettingsPage />;
    case "input":
      return <InputPage />;
    case "screen":
      return <ComingSoonPage key={route.page} feature={feature(route.page)} />;
    default:
      return <HomePage />;
  }
}

export default function App() {
  const [splash, setSplash] = useState(true);
  return (
    <AppProvider>
      <Shell>
        <CurrentPage />
      </Shell>
      <WhatsNew />
      <ConnectionRequestDialog />
      <Toasts />
      {splash && <Splash onDone={() => setSplash(false)} />}
    </AppProvider>
  );
}
