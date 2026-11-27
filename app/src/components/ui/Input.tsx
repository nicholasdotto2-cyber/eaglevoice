import React from "react";

interface InputProps extends React.InputHTMLAttributes<HTMLInputElement> {
  variant?: "default" | "compact";
}

export const Input: React.FC<InputProps> = ({
  className = "",
  variant = "default",
  disabled,
  ...props
}) => {
  const baseClasses =
    "px-2 py-1 text-sm font-medium bg-background border border-mid-gray/30 rounded-[10px] text-start transition-all duration-150";

  const interactiveClasses = disabled
    ? "opacity-60 cursor-not-allowed bg-mid-gray/10 border-mid-gray/40"
    : "hover:border-logo-primary/60 focus:outline-none focus:border-logo-primary focus:ring-2 focus:ring-accent/30";

  const variantClasses = {
    default: "px-3 py-2",
    compact: "px-2 py-1",
  } as const;

  return (
    <input
      className={`${baseClasses} ${variantClasses[variant]} ${interactiveClasses} ${className}`}
      disabled={disabled}
      {...props}
    />
  );
};
