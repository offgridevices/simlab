// Colour-scale stops: data, not design choices (which scale shows which
// result is chosen in tokens.css). Hex values are allowed only here and in
// tokens.css.

/** viridis (van der Walt & Smith, CC0): perceptually uniform, for magnitudes */
export const VIRIDIS = ["#440154", "#482878", "#3e4989", "#31688e", "#26828e", "#1f9e89", "#35b779", "#6ece58", "#fde725"];

/** cividis (Nuñez, Anderton & Renslow 2018): uniform and readable with colour blindness, for movement */
export const CIVIDIS = ["#00224e", "#123570", "#3b496c", "#575d6d", "#707173", "#8a8678", "#a59c74", "#c3b369", "#fee838"];

/** Moreland's cool-warm diverging scale: blue below zero, grey at zero, red above */
export const COOLWARM = ["#3b4cc0", "#6788ee", "#9abbff", "#c9d7f0", "#dddcdc", "#f2cbb7", "#f7a889", "#e26952", "#b40426"];

/** used only when the page's tokens cannot be read (tests, before styles load) */
export const STATUS_FALLBACK = { clay: "#f07a5a", gold: "#f0b84a", moss: "#8cc084", nan: "#6b7078" };
