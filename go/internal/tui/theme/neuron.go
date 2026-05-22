package theme

import (
	"github.com/charmbracelet/lipgloss"
)

// NeuronTheme implements the Theme interface with Neuron brand colors.
// Colors sourced from NeuronCLI brand.rs:
//   Primary Blue:  #4169C3
//   Accent Orange: #F0A028
//   Success Green: #2D8C3C
//   Error Red:     #C83228
//   Dim Grey:      #888888
//   Bright White:  #DCDCE6
//   Soft White:    #C8C8DC
//   Cyan Code:     #5AC8FA
type NeuronTheme struct {
	BaseTheme
}

func NewNeuronTheme() *NeuronTheme {
	// Neuron brand palette — dark mode
	darkBackground := "#1A1A2E"
	darkCurrentLine := "#1E1E32"
	darkSelection := "#2A2A40"
	darkForeground := "#DCDCE6"
	darkComment := "#888888"
	darkPrimary := "#4169C3"   // Brand blue
	darkSecondary := "#F0A028" // Brand orange
	darkAccent := "#5AC8FA"    // Brand cyan
	darkRed := "#C83228"       // Brand red
	darkOrange := "#F0A028"    // Brand orange
	darkGreen := "#2D8C3C"     // Brand green
	darkCyan := "#5AC8FA"      // Brand cyan
	darkYellow := "#E5C07B"
	darkBorder := "#4169C3" // Brand blue borders

	// Light mode
	lightBackground := "#F8F8FC"
	lightCurrentLine := "#F0F0F4"
	lightSelection := "#E5E5EA"
	lightForeground := "#2A2A3E"
	lightComment := "#8A8A9A"
	lightPrimary := "#3B5DAA"
	lightSecondary := "#D08820"
	lightAccent := "#2E8BC0"
	lightRed := "#C83228"
	lightOrange := "#D08820"
	lightGreen := "#2D8C3C"
	lightCyan := "#2E8BC0"
	lightYellow := "#B0851F"
	lightBorder := "#B8B8CC"

	theme := &NeuronTheme{}

	// Base colors
	theme.PrimaryColor = lipgloss.AdaptiveColor{Dark: darkPrimary, Light: lightPrimary}
	theme.SecondaryColor = lipgloss.AdaptiveColor{Dark: darkSecondary, Light: lightSecondary}
	theme.AccentColor = lipgloss.AdaptiveColor{Dark: darkAccent, Light: lightAccent}

	// Status colors
	theme.ErrorColor = lipgloss.AdaptiveColor{Dark: darkRed, Light: lightRed}
	theme.WarningColor = lipgloss.AdaptiveColor{Dark: darkOrange, Light: lightOrange}
	theme.SuccessColor = lipgloss.AdaptiveColor{Dark: darkGreen, Light: lightGreen}
	theme.InfoColor = lipgloss.AdaptiveColor{Dark: darkCyan, Light: lightCyan}

	// Text colors
	theme.TextColor = lipgloss.AdaptiveColor{Dark: darkForeground, Light: lightForeground}
	theme.TextMutedColor = lipgloss.AdaptiveColor{Dark: darkComment, Light: lightComment}
	theme.TextEmphasizedColor = lipgloss.AdaptiveColor{Dark: darkYellow, Light: lightYellow}

	// Background colors
	theme.BackgroundColor = lipgloss.AdaptiveColor{Dark: darkBackground, Light: lightBackground}
	theme.BackgroundSecondaryColor = lipgloss.AdaptiveColor{Dark: darkCurrentLine, Light: lightCurrentLine}
	theme.BackgroundDarkerColor = lipgloss.AdaptiveColor{Dark: "#121220", Light: "#FFFFFF"}

	// Border colors
	theme.BorderNormalColor = lipgloss.AdaptiveColor{Dark: darkBorder, Light: lightBorder}
	theme.BorderFocusedColor = lipgloss.AdaptiveColor{Dark: darkPrimary, Light: lightPrimary}
	theme.BorderDimColor = lipgloss.AdaptiveColor{Dark: darkSelection, Light: lightSelection}

	// Diff view colors
	theme.DiffAddedColor = lipgloss.AdaptiveColor{Dark: "#478247", Light: "#2E7D32"}
	theme.DiffRemovedColor = lipgloss.AdaptiveColor{Dark: "#7C4444", Light: "#C62828"}
	theme.DiffContextColor = lipgloss.AdaptiveColor{Dark: "#a0a0a0", Light: "#757575"}
	theme.DiffHunkHeaderColor = lipgloss.AdaptiveColor{Dark: "#a0a0a0", Light: "#757575"}
	theme.DiffHighlightAddedColor = lipgloss.AdaptiveColor{Dark: "#DAFADA", Light: "#A5D6A7"}
	theme.DiffHighlightRemovedColor = lipgloss.AdaptiveColor{Dark: "#FADADD", Light: "#EF9A9A"}
	theme.DiffAddedBgColor = lipgloss.AdaptiveColor{Dark: "#303A30", Light: "#E8F5E9"}
	theme.DiffRemovedBgColor = lipgloss.AdaptiveColor{Dark: "#3A3030", Light: "#FFEBEE"}
	theme.DiffContextBgColor = lipgloss.AdaptiveColor{Dark: darkBackground, Light: lightBackground}
	theme.DiffLineNumberColor = lipgloss.AdaptiveColor{Dark: "#888888", Light: "#9E9E9E"}
	theme.DiffAddedLineNumberBgColor = lipgloss.AdaptiveColor{Dark: "#293229", Light: "#C8E6C9"}
	theme.DiffRemovedLineNumberBgColor = lipgloss.AdaptiveColor{Dark: "#332929", Light: "#FFCDD2"}

	// Markdown colors
	theme.MarkdownTextColor = lipgloss.AdaptiveColor{Dark: darkForeground, Light: lightForeground}
	theme.MarkdownHeadingColor = lipgloss.AdaptiveColor{Dark: darkPrimary, Light: lightPrimary}
	theme.MarkdownLinkColor = lipgloss.AdaptiveColor{Dark: darkSecondary, Light: lightSecondary}
	theme.MarkdownLinkTextColor = lipgloss.AdaptiveColor{Dark: darkCyan, Light: lightCyan}
	theme.MarkdownCodeColor = lipgloss.AdaptiveColor{Dark: darkGreen, Light: lightGreen}
	theme.MarkdownBlockQuoteColor = lipgloss.AdaptiveColor{Dark: darkYellow, Light: lightYellow}
	theme.MarkdownEmphColor = lipgloss.AdaptiveColor{Dark: darkYellow, Light: lightYellow}
	theme.MarkdownStrongColor = lipgloss.AdaptiveColor{Dark: darkAccent, Light: lightAccent}
	theme.MarkdownHorizontalRuleColor = lipgloss.AdaptiveColor{Dark: darkComment, Light: lightComment}
	theme.MarkdownListItemColor = lipgloss.AdaptiveColor{Dark: darkSecondary, Light: lightSecondary}
	theme.MarkdownListEnumerationColor = lipgloss.AdaptiveColor{Dark: darkCyan, Light: lightCyan}
	theme.MarkdownImageColor = lipgloss.AdaptiveColor{Dark: darkSecondary, Light: lightSecondary}
	theme.MarkdownImageTextColor = lipgloss.AdaptiveColor{Dark: darkCyan, Light: lightCyan}
	theme.MarkdownCodeBlockColor = lipgloss.AdaptiveColor{Dark: darkForeground, Light: lightForeground}

	// Syntax highlighting colors
	theme.SyntaxCommentColor = lipgloss.AdaptiveColor{Dark: darkComment, Light: lightComment}
	theme.SyntaxKeywordColor = lipgloss.AdaptiveColor{Dark: darkPrimary, Light: lightPrimary}
	theme.SyntaxFunctionColor = lipgloss.AdaptiveColor{Dark: darkSecondary, Light: lightSecondary}
	theme.SyntaxVariableColor = lipgloss.AdaptiveColor{Dark: darkRed, Light: lightRed}
	theme.SyntaxStringColor = lipgloss.AdaptiveColor{Dark: darkGreen, Light: lightGreen}
	theme.SyntaxNumberColor = lipgloss.AdaptiveColor{Dark: darkAccent, Light: lightAccent}
	theme.SyntaxTypeColor = lipgloss.AdaptiveColor{Dark: darkYellow, Light: lightYellow}
	theme.SyntaxOperatorColor = lipgloss.AdaptiveColor{Dark: darkCyan, Light: lightCyan}
	theme.SyntaxPunctuationColor = lipgloss.AdaptiveColor{Dark: darkForeground, Light: lightForeground}

	return theme
}

func init() {
	RegisterTheme("neuron", NewNeuronTheme())
}
