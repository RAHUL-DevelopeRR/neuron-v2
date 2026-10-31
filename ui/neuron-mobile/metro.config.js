// The shared client lives beside this app (ui/neuron-client); Metro watches it
// and resolves `@neuron/client` to its source.
const path = require('path')
const { getDefaultConfig } = require('expo/metro-config')

const config = getDefaultConfig(__dirname)
const client = path.resolve(__dirname, '../neuron-client')
config.watchFolders = [client]
config.resolver.extraNodeModules = { '@neuron/client': path.join(client, 'src') }
config.resolver.nodeModulesPaths = [path.resolve(__dirname, 'node_modules')]
module.exports = config
