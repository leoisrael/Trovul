// Scene chips: one click puts a saved mix back on both PCs. In the Mixer they
// can also be saved, updated with the current mix and deleted; the tray mixer
// only applies them.

import { useEffect, useRef, useState } from "react";
import { Check, Ellipsis, Plus, RefreshCw, Trash2 } from "lucide-react";
import { errorMessage } from "../../api";
import { applyScene, captureScene, sceneIcon, useScenes } from "../../app/scenes";
import type { MixScene } from "../../types";
import { Button } from "../../ui";

const MAX_SCENES = 12;

export function SceneChips({
  linked,
  editable,
  onMessage,
  onError,
}: {
  linked: boolean;
  editable?: boolean;
  onMessage: (message: string) => void;
  onError: (message: string) => void;
}) {
  const [scenes, save] = useScenes();
  const [busy, setBusy] = useState<string | null>(null);
  const [applied, setApplied] = useState<string | null>(null);
  // The chip whose Update / Delete buttons are showing (opened with "⋯").
  const [editing, setEditing] = useState<string | null>(null);
  const [naming, setNaming] = useState(false);
  const timer = useRef<number | undefined>(undefined);

  useEffect(() => () => window.clearTimeout(timer.current), []);

  if (!scenes) return null;
  if (!editable && scenes.length === 0) return null;

  const apply = async (scene: MixScene) => {
    setBusy(scene.id);
    try {
      onMessage(await applyScene(scene, linked));
      setApplied(scene.id);
      window.clearTimeout(timer.current);
      timer.current = window.setTimeout(() => setApplied(null), 1600);
    } catch (e) {
      onError(`Scene: ${errorMessage(e)}`);
    } finally {
      setBusy(null);
    }
  };

  const overwrite = async (scene: MixScene) => {
    setEditing(null);
    setBusy(scene.id);
    try {
      const fresh = await captureScene(scene.name, linked, scene.id);
      await save(scenes.map((s) => (s.id === scene.id ? fresh : s)));
      onMessage(`“${scene.name}” now has the current mix`);
    } catch (e) {
      onError(`Scene: ${errorMessage(e)}`);
    } finally {
      setBusy(null);
    }
  };

  const remove = async (scene: MixScene) => {
    setEditing(null);
    await save(scenes.filter((s) => s.id !== scene.id)).catch((e) => onError(`Scene: ${errorMessage(e)}`));
  };

  const create = async (name: string) => {
    setNaming(false);
    try {
      const scene = await captureScene(name, linked);
      await save([...scenes, scene]);
      onMessage(`Saved “${scene.name}”${scene.other_pc ? " for both PCs" : ""}`);
    } catch (e) {
      onError(`Scene: ${errorMessage(e)}`);
    }
  };

  return (
    <div className="scenes" role="group" aria-label="Scenes">
      {scenes.map((scene) => {
        const Icon = sceneIcon(scene.name);
        return (
          <div
            key={scene.id}
            className={`scene${applied === scene.id ? " applied" : ""}${editing === scene.id ? " editing" : ""}`}
            onMouseLeave={() => editing === scene.id && setEditing(null)}
          >
            <button
              type="button"
              className="scene-apply"
              disabled={busy !== null}
              title={`Apply “${scene.name}”${scene.other_pc ? " on both PCs" : ""}`}
              onClick={() => apply(scene)}
            >
              {applied === scene.id ? <Check /> : <Icon />}
              <span>{scene.name}</span>
            </button>
            {editable &&
              (editing === scene.id ? (
                <span className="scene-actions">
                  <button
                    type="button"
                    title={`Save the current mix into “${scene.name}”`}
                    disabled={busy !== null}
                    onClick={() => overwrite(scene)}
                  >
                    <RefreshCw />
                    Update
                  </button>
                  <button type="button" className="danger" title={`Delete “${scene.name}”`} onClick={() => remove(scene)}>
                    <Trash2 />
                    Delete
                  </button>
                </span>
              ) : (
                <button
                  type="button"
                  className="scene-more"
                  title={`Update or delete “${scene.name}”`}
                  aria-label={`Edit ${scene.name}`}
                  onClick={() => setEditing(scene.id)}
                >
                  <Ellipsis />
                </button>
              ))}
          </div>
        );
      })}
      {editable &&
        (naming ? (
          <NameField defaultName={`Scene ${scenes.length + 1}`} onSave={create} onCancel={() => setNaming(false)} />
        ) : (
          scenes.length < MAX_SCENES && (
            <Button size="sm" icon={Plus} className="scene-new" onClick={() => setNaming(true)}>
              {scenes.length === 0 ? "Save this mix as a scene" : "Save mix"}
            </Button>
          )
        ))}
      {editable && scenes.length === 0 && !naming && (
        <span className="scenes-hint">Volumes, mutes and outputs of both PCs, back with one click.</span>
      )}
    </div>
  );
}

function NameField({ defaultName, onSave, onCancel }: { defaultName: string; onSave: (name: string) => void; onCancel: () => void }) {
  const [name, setName] = useState(defaultName);
  const input = useRef<HTMLInputElement>(null);
  useEffect(() => input.current?.select(), []);
  const submit = () => (name.trim() ? onSave(name.trim()) : onCancel());
  return (
    <form
      className="scene-form"
      onSubmit={(e) => {
        e.preventDefault();
        submit();
      }}
    >
      <input
        ref={input}
        className="input"
        value={name}
        maxLength={32}
        aria-label="Scene name"
        placeholder="Game, Stream, Meeting…"
        onChange={(e) => setName(e.target.value)}
        onKeyDown={(e) => e.key === "Escape" && onCancel()}
      />
      <Button size="sm" variant="primary" type="submit">
        Save
      </Button>
      <Button size="sm" onClick={onCancel}>
        Cancel
      </Button>
    </form>
  );
}
