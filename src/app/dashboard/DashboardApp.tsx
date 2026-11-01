import { AlertTriangle, Library, PanelTop, Settings } from "lucide-react";
import { useEffect } from "react";
import { BrowserRouter, Navigate, NavLink, Route, Routes, useLocation, useNavigate } from "react-router-dom";
import { TooltipProvider } from "@/components/ui/tooltip";
import { native } from "@/core/native";
import { initSettings, useKeys, useSettings } from "@/core/settings";
import { cn } from "@/lib/utils";
import { Onboarding } from "./Onboarding";
import { SessionsPage } from "./SessionsPage";
import { SettingsPage } from "./SettingsPage";

export function DashboardApp() {
  useEffect(() => {
    void initSettings();
  }, []);

  return (
    <BrowserRouter>
      <TooltipProvider>
        <NavigateListener />
        <Shell />
      </TooltipProvider>
    </BrowserRouter>
  );
}

/** dashboard_open(route) on an existing dashboard window navigates instead of reloading. */
function NavigateListener() {
  const navigate = useNavigate();
  useEffect(() => {
    let un: (() => void) | undefined;
    void native.on("dashboard://navigate", ({ route }) => navigate(route)).then((u) => (un = u));
    return () => un?.();
  }, [navigate]);
  return null;
}

function Shell() {
  const loaded = useSettings((s) => s.loaded);
  const onboarded = useSettings((s) => s.settings.onboarded);
  const location = useLocation();
  const onOnboarding = location.pathname.startsWith("/dashboard/onboarding");

  if (!loaded) return <div className="h-full" aria-busy />;
  if (!onboarded && !onOnboarding) return <Navigate to="/dashboard/onboarding" replace />;
  if (onOnboarding) {
    return (
      <div className="h-full">
        <Onboarding />
      </div>
    );
  }

  return (
    <div className="flex h-full">
      <Sidebar />
      <main className="min-w-0 flex-1">
        <Routes>
          <Route path="/dashboard/sessions" element={<SessionsPage />} />
          <Route path="/dashboard/sessions/:id" element={<SessionsPage />} />
          <Route path="/dashboard/settings" element={<SettingsPage />} />
          <Route path="*" element={<Navigate to="/dashboard/sessions" replace />} />
        </Routes>
      </main>
    </div>
  );
}

function Sidebar() {
  const keys = useKeys((s) => s.status);
  const missingClaude = keys && !keys.anthropic.present;
  const missingDeepgram = keys && !keys.deepgram.present;

  const link = ({ isActive }: { isActive: boolean }) =>
    cn(
      "flex items-center gap-2.5 rounded-lg px-2.5 py-2 text-[14px] font-medium transition-colors",
      isActive ? "bg-accent text-foreground" : "text-muted-foreground hover:bg-accent/60 hover:text-foreground",
    );

  return (
    <nav aria-label="Dashboard" className="flex w-[208px] shrink-0 flex-col border-r border-border bg-raised px-3 py-4">
      <div className="mb-5 flex items-center gap-2 px-1.5">
        <span className="flex size-7 items-center justify-center rounded-lg bg-primary text-[14px] font-bold text-primary-foreground">X</span>
        <span className="text-[15px] font-semibold">Pluely X</span>
      </div>
      <div className="flex flex-col gap-0.5">
        <NavLink to="/dashboard/sessions" className={link}>
          <Library className="size-4" /> Sessions
        </NavLink>
        <NavLink to="/dashboard/settings" className={link}>
          <Settings className="size-4" /> Settings
        </NavLink>
      </div>
      <span className="flex-1" />
      {missingClaude || missingDeepgram ? (
        <NavLink
          to={missingClaude ? "/dashboard/onboarding" : "/dashboard/settings#keys"}
          className="mb-3 flex gap-2 rounded-lg border border-warn/30 bg-warn/10 p-2.5 text-[12.5px] leading-snug text-foreground hover:bg-warn/15"
        >
          <AlertTriangle className="mt-0.5 size-4 shrink-0 text-warn" />
          {missingClaude ? "Add your Claude key to get answers." : "Add a Deepgram key to turn on listening."}
        </NavLink>
      ) : null}
      <button
        type="button"
        onClick={() => void native.windowShow().catch(() => {})}
        className="flex items-center gap-2.5 rounded-lg px-2.5 py-2 text-[13.5px] text-muted-foreground transition-colors hover:bg-accent/60 hover:text-foreground"
      >
        <PanelTop className="size-4" /> Show overlay
      </button>
    </nav>
  );
}
