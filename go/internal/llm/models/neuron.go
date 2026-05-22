package models

// NeuronCLI Gateway models — served via zero-x.live auth server.
// These models are accessed through the gateway proxy; no API keys needed on client.

const ProviderNeuron ModelProvider = "neuron"

// Neuron model IDs
const (
	NeuronKimiK25          ModelID = "neuron.kimi-k2.5"
	NeuronKimiK26          ModelID = "neuron.kimi-k2.6"
	NeuronDeepSeekV4Flash  ModelID = "neuron.deepseek-v4-flash"
	NeuronDeepSeekV32      ModelID = "neuron.deepseek-v3.2"
	NeuronMiniMaxM25       ModelID = "neuron.minimax-m2.5"
	NeuronModelRouter      ModelID = "neuron.model-router"
	NeuronGPT54Pro         ModelID = "neuron.gpt-5.4-pro"
	NeuronGPT54Mini        ModelID = "neuron.gpt-5.4-mini"
	NeuronGPT55            ModelID = "neuron.gpt-5.5-2"
	NeuronCodexMax         ModelID = "neuron.codex-max"
)

var NeuronModels = map[ModelID]Model{
	NeuronKimiK25: {
		ID:               NeuronKimiK25,
		Name:             "Kimi K2.5",
		Provider:         ProviderNeuron,
		APIModel:         "Kimi-K2.5",
		ContextWindow:    131072,
		DefaultMaxTokens: 16384,
		CanReason:        true,
	},
	NeuronKimiK26: {
		ID:               NeuronKimiK26,
		Name:             "Kimi K2.6",
		Provider:         ProviderNeuron,
		APIModel:         "Kimi-K2.6",
		ContextWindow:    131072,
		DefaultMaxTokens: 16384,
		CanReason:        true,
	},
	NeuronDeepSeekV4Flash: {
		ID:               NeuronDeepSeekV4Flash,
		Name:             "DeepSeek V4 Flash",
		Provider:         ProviderNeuron,
		APIModel:         "DeepSeek-V4-Flash",
		ContextWindow:    131072,
		DefaultMaxTokens: 16384,
		CanReason:        true,
	},
	NeuronDeepSeekV32: {
		ID:               NeuronDeepSeekV32,
		Name:             "DeepSeek V3.2",
		Provider:         ProviderNeuron,
		APIModel:         "FW-DeepSeek-V3.2",
		ContextWindow:    131072,
		DefaultMaxTokens: 16384,
		CanReason:        false,
	},
	NeuronMiniMaxM25: {
		ID:               NeuronMiniMaxM25,
		Name:             "MiniMax M2.5",
		Provider:         ProviderNeuron,
		APIModel:         "FW-MiniMax-M2.5",
		ContextWindow:    131072,
		DefaultMaxTokens: 16384,
		CanReason:        false,
	},
	NeuronModelRouter: {
		ID:               NeuronModelRouter,
		Name:             "Model Router (Auto)",
		Provider:         ProviderNeuron,
		APIModel:         "model-router",
		ContextWindow:    131072,
		DefaultMaxTokens: 16384,
		CanReason:        false,
	},
	NeuronGPT54Pro: {
		ID:               NeuronGPT54Pro,
		Name:             "GPT-5.4 Pro",
		Provider:         ProviderNeuron,
		APIModel:         "gpt-5.4-pro",
		ContextWindow:    131072,
		DefaultMaxTokens: 16384,
		CanReason:        true,
	},
	NeuronGPT54Mini: {
		ID:               NeuronGPT54Mini,
		Name:             "GPT-5.4 Mini",
		Provider:         ProviderNeuron,
		APIModel:         "gpt-5.4-mini",
		ContextWindow:    131072,
		DefaultMaxTokens: 16384,
		CanReason:        false,
	},
	NeuronGPT55: {
		ID:               NeuronGPT55,
		Name:             "GPT-5.5",
		Provider:         ProviderNeuron,
		APIModel:         "gpt-5.5-2",
		ContextWindow:    131072,
		DefaultMaxTokens: 16384,
		CanReason:        true,
	},
	NeuronCodexMax: {
		ID:               NeuronCodexMax,
		Name:             "Codex Max",
		Provider:         ProviderNeuron,
		APIModel:         "gpt-5.1-codex-max",
		ContextWindow:    131072,
		DefaultMaxTokens: 16384,
		CanReason:        true,
	},
}
