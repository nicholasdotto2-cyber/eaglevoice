import React, { useEffect } from "react";
import { Button } from "../ui/Button";
import { Dropdown, DropdownOption } from "../ui/Dropdown";
import { PlayIcon } from "lucide-react";
import { SettingContainer } from "../ui/SettingContainer";
import { useSettingsStore } from "../../stores/settingsStore";
import { useSettings } from "../../hooks/useSettings";
import { previewSoundTheme } from "../../lib/sounds";

interface SoundPickerProps {
  label: string;
  description: string;
  /** Testo del pulsante di anteprima; senza, resta quello di Handy. */
  previewLabel?: string;
}

export const SoundPicker: React.FC<SoundPickerProps> = ({
  label,
  description,
  previewLabel = "Preview sound theme (plays start then stop)",
}) => {
  const { getSetting, updateSetting } = useSettings();
  const playTestSound = useSettingsStore((state) => state.playTestSound);
  const customSounds = useSettingsStore((state) => state.customSounds);
  const checkCustomSounds = useSettingsStore(
    (state) => state.checkCustomSounds,
  );

  const selectedTheme = getSetting("sound_theme") ?? "marimba";

  useEffect(() => {
    checkCustomSounds();
  }, [checkCustomSounds]);

  const options: DropdownOption[] = [
    { value: "marimba", label: "Marimba" },
    { value: "pop", label: "Pop" },
  ];

  // Only add Custom option if both custom sound files exist
  if (customSounds.start && customSounds.stop) {
    options.push({ value: "custom", label: "Custom" });
  }

  const handlePlayBothSounds = () => previewSoundTheme(playTestSound);

  return (
    <SettingContainer
      title={label}
      description={description}
      grouped
      layout="horizontal"
    >
      <div className="flex items-center gap-2">
        <Dropdown
          selectedValue={selectedTheme}
          onSelect={(value) =>
            updateSetting("sound_theme", value as "marimba" | "pop" | "custom")
          }
          options={options}
          onOpen={checkCustomSounds}
        />
        <Button
          variant="ghost"
          size="sm"
          onClick={handlePlayBothSounds}
          title={previewLabel}
          aria-label={previewLabel}
        >
          <PlayIcon className="h-4 w-4" />
        </Button>
      </div>
    </SettingContainer>
  );
};
