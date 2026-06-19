package models

// NeuronCLI Gateway models are served through zero-x.live.
// APIModel must match the model IDs advertised by the gateway.

const ProviderNeuron ModelProvider = "neuron"

const (
	NeuronKimiK25         ModelID = "neuron.kimi-k2.5"
	NeuronKimiK26         ModelID = "neuron.kimi-k2.6"
	NeuronDeepSeekV4Flash ModelID = "neuron.deepseek-v4-flash"
	NeuronDeepSeekV32     ModelID = "neuron.deepseek-v3.2"
	NeuronMiniMaxM25      ModelID = "neuron.minimax-m2.5"
	NeuronModelRouter     ModelID = "neuron.model-router"
	NeuronGPT54Pro        ModelID = "neuron.gpt-5.4-pro"
	NeuronGPT54Mini       ModelID = "neuron.gpt-5.4-mini"
	NeuronGPT55           ModelID = "neuron.gpt-5.5-2"
	NeuronCodexMax        ModelID = "neuron.codex-max"
	NeuronQwen3CoderFree  ModelID = "neuron.qwen3-coder-free"
)

var NeuronModels = map[ModelID]Model{
	NeuronModelRouter: {
		ID:                  NeuronModelRouter,
		Name:                "Model Router",
		Provider:            ProviderNeuron,
		APIModel:            "model-router",
		ContextWindow:       131072,
		DefaultMaxTokens:    16384,
		CanReason:           false,
		SupportsAttachments: true,
	},
	NeuronKimiK25: {
		ID:                  NeuronKimiK25,
		Name:                "Kimi K2.5",
		Provider:            ProviderNeuron,
		APIModel:            "Kimi-K2.5",
		ContextWindow:       131072,
		DefaultMaxTokens:    16384,
		CanReason:           true,
		SupportsAttachments: true,
	},
	NeuronKimiK26: {
		ID:                  NeuronKimiK26,
		Name:                "Kimi K2.6",
		Provider:            ProviderNeuron,
		APIModel:            "Kimi-K2.6",
		ContextWindow:       131072,
		DefaultMaxTokens:    16384,
		CanReason:           true,
		SupportsAttachments: true,
	},
	NeuronDeepSeekV32: {
		ID:                  NeuronDeepSeekV32,
		Name:                "DeepSeek V3.2",
		Provider:            ProviderNeuron,
		APIModel:            "FW-DeepSeek-V3.2",
		ContextWindow:       131072,
		DefaultMaxTokens:    16384,
		CanReason:           false,
		SupportsAttachments: true,
	},
	NeuronMiniMaxM25: {
		ID:                  NeuronMiniMaxM25,
		Name:                "MiniMax M2.5",
		Provider:            ProviderNeuron,
		APIModel:            "FW-MiniMax-M2.5",
		ContextWindow:       131072,
		DefaultMaxTokens:    16384,
		CanReason:           false,
		SupportsAttachments: true,
	},
	NeuronGPT55: {
		ID:                  NeuronGPT55,
		Name:                "GPT-5.5",
		Provider:            ProviderNeuron,
		APIModel:            "gpt-5.5",
		ContextWindow:       131072,
		DefaultMaxTokens:    16384,
		CanReason:           true,
		SupportsAttachments: true,
	},
	NeuronCodexMax: {
		ID:                  NeuronCodexMax,
		Name:                "Codex Max",
		Provider:            ProviderNeuron,
		APIModel:            "Codex-Max",
		ContextWindow:       131072,
		DefaultMaxTokens:    16384,
		CanReason:           true,
		SupportsAttachments: true,
	},
	NeuronQwen3CoderFree: {
		ID:                  NeuronQwen3CoderFree,
		Name:                "Qwen3 Coder Free",
		Provider:            ProviderNeuron,
		APIModel:            "qwen/qwen3-coder-480b-a35b-instruct:free",
		ContextWindow:       262144,
		DefaultMaxTokens:    16384,
		CanReason:           false,
		SupportsAttachments: true,
	},
}
