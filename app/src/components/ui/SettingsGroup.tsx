import React from "react";

interface SettingsGroupProps {
  title?: string;
  description?: string;
  children: React.ReactNode;
}

export const SettingsGroup: React.FC<SettingsGroupProps> = ({
  title,
  description,
  children,
}) => {
  return (
    <div className="space-y-2.5">
      {title && (
        <div className="px-1">
          {/* Titolo tecnico: monospace, maiuscolo, spaziato. */}
          <h2 className="ev-label text-text">{title}</h2>
          {description && (
            <p className="text-xs text-mid-gray mt-0.5">{description}</p>
          )}
        </div>
      )}
      <div className="ev-corners bg-surface border border-hairline rounded-[14px] overflow-visible">
        <div className="divide-y divide-hairline">{children}</div>
      </div>
    </div>
  );
};
