import type { ComponentProps } from "react";
import type { LucideIcon } from "lucide-react";

// React 19: `ref` is a regular prop and reaches the <button> through `rest`.
interface ButtonProps extends ComponentProps<"button"> {
  variant?: "primary" | "secondary" | "ghost" | "danger";
  size?: "sm" | "md" | "lg";
  icon?: LucideIcon;
  /** Square button with just the icon; `title` becomes its accessible name. */
  iconOnly?: boolean;
  loading?: boolean;
}

export function Button({
  variant = "secondary",
  size = "md",
  icon: Icon,
  iconOnly,
  loading,
  className,
  children,
  disabled,
  type = "button",
  ...rest
}: ButtonProps) {
  const classes = [
    "btn",
    variant !== "secondary" && `btn-${variant}`,
    size !== "md" && `btn-${size}`,
    iconOnly && "btn-icon",
    className,
  ]
    .filter(Boolean)
    .join(" ");
  return (
    <button
      type={type}
      className={classes}
      disabled={disabled || loading}
      aria-label={iconOnly ? rest.title : undefined}
      {...rest}
    >
      {loading ? <span className="spinner" aria-hidden="true" /> : Icon && <Icon aria-hidden="true" />}
      {!iconOnly && children}
    </button>
  );
}
