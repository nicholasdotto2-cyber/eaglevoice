import React from "react";

interface ButtonProps extends React.ButtonHTMLAttributes<HTMLButtonElement> {
  variant?:
    | "primary"
    | "primary-soft"
    | "secondary"
    | "warning"
    | "danger"
    | "danger-ghost"
    | "ghost";
  size?: "sm" | "md" | "lg";
}

export const Button: React.FC<ButtonProps> = ({
  children,
  className = "",
  variant = "primary",
  size = "md",
  ...props
}) => {
  const baseClasses =
    "font-medium rounded-[10px] border focus:outline-none transition-colors disabled:opacity-50 disabled:cursor-not-allowed cursor-pointer";

  const variantClasses = {
    primary:
      "text-on-primary bg-background-ui border-background-ui hover:bg-background-ui/90 hover:border-background-ui/90 focus:ring-2 focus:ring-accent/40",
    "primary-soft":
      "text-logo-primary bg-logo-primary/10 border-transparent hover:bg-logo-primary/15 focus:ring-2 focus:ring-accent/40",
    secondary:
      "text-text bg-background border-mid-gray/30 hover:bg-logo-primary/5 hover:border-logo-primary/60 focus:outline-none",
    // Secondary's neutral resting look, but hover/focus use the semantic
    // --color-warning token (theme.css) instead of the blue accent — for
    // buttons sitting on warning surfaces like SecureInputWarning
    warning:
      "text-text bg-mid-gray/10 border-hairline hover:bg-warning/15 hover:border-warning focus:ring-1 focus:ring-warning",
    danger:
      "text-white bg-red-600 border-hairline hover:bg-red-700 hover:border-red-700 focus:ring-1 focus:ring-red-500",
    "danger-ghost":
      "text-error border-transparent hover:bg-error/10 focus:bg-error/15",
    ghost:
      "text-current border-transparent hover:bg-mid-gray/10 focus:bg-mid-gray/15",
  };

  const sizeClasses = {
    sm: "px-2 py-1 text-xs",
    md: "px-4 py-[5px] text-sm",
    lg: "px-4 py-2 text-base",
  };

  return (
    <button
      className={`${baseClasses} ${variantClasses[variant]} ${sizeClasses[size]} ${className}`}
      {...props}
    >
      {children}
    </button>
  );
};
