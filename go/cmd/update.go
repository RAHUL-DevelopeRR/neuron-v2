package cmd

import (
	"fmt"

	"github.com/spf13/cobra"
)

func newUpdateCommand() *cobra.Command {
	var download bool
	cmd := &cobra.Command{
		Use:   "update",
		Short: "Show the correct update path for this NeuronCLI install",
		RunE: func(cmd *cobra.Command, args []string) error {
			return runUpdate(download)
		},
	}
	cmd.Flags().BoolVar(&download, "download", false, "Use the GitHub Release asset flow for archive installs")
	return cmd
}

func runUpdate(download bool) error {
	info := detectInstallInfo()
	fmt.Printf("Install source: %s\n", info.Source)
	fmt.Printf("Recommended update: %s\n", info.UpdateCommand)
	if download {
		fmt.Println("Release assets: https://github.com/RAHUL-DevelopeRR/neuron-v2/releases/latest")
		fmt.Println("Download the matching signed asset for your OS/arch, verify checksums.txt, and replace the current binary.")
	}
	return nil
}
