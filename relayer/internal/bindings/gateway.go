// Code generated - DO NOT EDIT.
// This file is a generated binding and any manual changes will be lost.

package bindings

import (
	"errors"
	"math/big"
	"strings"

	ethereum "github.com/ethereum/go-ethereum"
	"github.com/ethereum/go-ethereum/accounts/abi"
	"github.com/ethereum/go-ethereum/accounts/abi/bind"
	"github.com/ethereum/go-ethereum/common"
	"github.com/ethereum/go-ethereum/core/types"
	"github.com/ethereum/go-ethereum/event"
)

// Reference imports to suppress errors if they are not otherwise used.
var (
	_ = errors.New
	_ = big.NewInt
	_ = strings.NewReader
	_ = ethereum.NotFound
	_ = bind.Bind
	_ = common.Big1
	_ = types.BloomLookup
	_ = event.NewSubscription
	_ = abi.ConvertType
)

// GatewayMessageProof is an auto generated low-level Go binding around an user-defined struct.
type GatewayMessageProof struct {
	Position *big.Int
	Width    *big.Int
	Proof    [][32]byte
}

// GatewayMmrLeaf is an auto generated low-level Go binding around an user-defined struct.
type GatewayMmrLeaf struct {
	Version              uint8
	ParentNumber         uint32
	ParentHash           [32]byte
	NextAuthoritySetID   uint64
	NextAuthoritySetLen  uint32
	NextAuthoritySetRoot [32]byte
	LeafExtra            [32]byte
}

// GatewayMmrLeafProof is an auto generated low-level Go binding around an user-defined struct.
type GatewayMmrLeafProof struct {
	Siblings [][32]byte
	Order    *big.Int
}

// OutboundMessage is an auto generated low-level Go binding around an user-defined struct.
type OutboundMessage struct {
	Nonce       uint64
	Destination common.Address
	Payload     []byte
}

// GatewayMetaData contains all meta data concerning the Gateway contract.
var GatewayMetaData = &bind.MetaData{
	ABI: "[{\"type\":\"constructor\",\"inputs\":[{\"name\":\"_beefyClient\",\"type\":\"address\",\"internalType\":\"contractBeefyClient\"}],\"stateMutability\":\"nonpayable\"},{\"type\":\"function\",\"name\":\"beefyClient\",\"inputs\":[],\"outputs\":[{\"name\":\"\",\"type\":\"address\",\"internalType\":\"contractBeefyClient\"}],\"stateMutability\":\"view\"},{\"type\":\"function\",\"name\":\"hashMessageLeaf\",\"inputs\":[{\"name\":\"m\",\"type\":\"tuple\",\"internalType\":\"structOutboundMessage\",\"components\":[{\"name\":\"nonce\",\"type\":\"uint64\",\"internalType\":\"uint64\"},{\"name\":\"destination\",\"type\":\"address\",\"internalType\":\"address\"},{\"name\":\"payload\",\"type\":\"bytes\",\"internalType\":\"bytes\"}]}],\"outputs\":[{\"name\":\"\",\"type\":\"bytes32\",\"internalType\":\"bytes32\"}],\"stateMutability\":\"pure\"},{\"type\":\"function\",\"name\":\"hashMmrLeaf\",\"inputs\":[{\"name\":\"leaf\",\"type\":\"tuple\",\"internalType\":\"structGateway.MmrLeaf\",\"components\":[{\"name\":\"version\",\"type\":\"uint8\",\"internalType\":\"uint8\"},{\"name\":\"parentNumber\",\"type\":\"uint32\",\"internalType\":\"uint32\"},{\"name\":\"parentHash\",\"type\":\"bytes32\",\"internalType\":\"bytes32\"},{\"name\":\"nextAuthoritySetID\",\"type\":\"uint64\",\"internalType\":\"uint64\"},{\"name\":\"nextAuthoritySetLen\",\"type\":\"uint32\",\"internalType\":\"uint32\"},{\"name\":\"nextAuthoritySetRoot\",\"type\":\"bytes32\",\"internalType\":\"bytes32\"},{\"name\":\"leafExtra\",\"type\":\"bytes32\",\"internalType\":\"bytes32\"}]}],\"outputs\":[{\"name\":\"\",\"type\":\"bytes32\",\"internalType\":\"bytes32\"}],\"stateMutability\":\"pure\"},{\"type\":\"function\",\"name\":\"inboundDelivered\",\"inputs\":[{\"name\":\"nonce\",\"type\":\"uint64\",\"internalType\":\"uint64\"}],\"outputs\":[{\"name\":\"\",\"type\":\"bool\",\"internalType\":\"bool\"}],\"stateMutability\":\"view\"},{\"type\":\"function\",\"name\":\"outboundNonce\",\"inputs\":[],\"outputs\":[{\"name\":\"\",\"type\":\"uint64\",\"internalType\":\"uint64\"}],\"stateMutability\":\"view\"},{\"type\":\"function\",\"name\":\"sendMessage\",\"inputs\":[{\"name\":\"payload\",\"type\":\"bytes\",\"internalType\":\"bytes\"}],\"outputs\":[{\"name\":\"nonce\",\"type\":\"uint64\",\"internalType\":\"uint64\"}],\"stateMutability\":\"nonpayable\"},{\"type\":\"function\",\"name\":\"submitInbound\",\"inputs\":[{\"name\":\"message\",\"type\":\"tuple\",\"internalType\":\"structOutboundMessage\",\"components\":[{\"name\":\"nonce\",\"type\":\"uint64\",\"internalType\":\"uint64\"},{\"name\":\"destination\",\"type\":\"address\",\"internalType\":\"address\"},{\"name\":\"payload\",\"type\":\"bytes\",\"internalType\":\"bytes\"}]},{\"name\":\"leaf\",\"type\":\"tuple\",\"internalType\":\"structGateway.MmrLeaf\",\"components\":[{\"name\":\"version\",\"type\":\"uint8\",\"internalType\":\"uint8\"},{\"name\":\"parentNumber\",\"type\":\"uint32\",\"internalType\":\"uint32\"},{\"name\":\"parentHash\",\"type\":\"bytes32\",\"internalType\":\"bytes32\"},{\"name\":\"nextAuthoritySetID\",\"type\":\"uint64\",\"internalType\":\"uint64\"},{\"name\":\"nextAuthoritySetLen\",\"type\":\"uint32\",\"internalType\":\"uint32\"},{\"name\":\"nextAuthoritySetRoot\",\"type\":\"bytes32\",\"internalType\":\"bytes32\"},{\"name\":\"leafExtra\",\"type\":\"bytes32\",\"internalType\":\"bytes32\"}]},{\"name\":\"leafProof\",\"type\":\"tuple\",\"internalType\":\"structGateway.MmrLeafProof\",\"components\":[{\"name\":\"siblings\",\"type\":\"bytes32[]\",\"internalType\":\"bytes32[]\"},{\"name\":\"order\",\"type\":\"uint256\",\"internalType\":\"uint256\"}]},{\"name\":\"msgProof\",\"type\":\"tuple\",\"internalType\":\"structGateway.MessageProof\",\"components\":[{\"name\":\"position\",\"type\":\"uint256\",\"internalType\":\"uint256\"},{\"name\":\"width\",\"type\":\"uint256\",\"internalType\":\"uint256\"},{\"name\":\"proof\",\"type\":\"bytes32[]\",\"internalType\":\"bytes32[]\"}]}],\"outputs\":[],\"stateMutability\":\"nonpayable\"},{\"type\":\"event\",\"name\":\"InboundMessageDispatched\",\"inputs\":[{\"name\":\"nonce\",\"type\":\"uint64\",\"indexed\":true,\"internalType\":\"uint64\"},{\"name\":\"destination\",\"type\":\"address\",\"indexed\":true,\"internalType\":\"address\"},{\"name\":\"success\",\"type\":\"bool\",\"indexed\":false,\"internalType\":\"bool\"}],\"anonymous\":false},{\"type\":\"event\",\"name\":\"OutboundMessageAccepted\",\"inputs\":[{\"name\":\"nonce\",\"type\":\"uint64\",\"indexed\":true,\"internalType\":\"uint64\"},{\"name\":\"origin\",\"type\":\"address\",\"indexed\":true,\"internalType\":\"address\"},{\"name\":\"payload\",\"type\":\"bytes\",\"indexed\":false,\"internalType\":\"bytes\"}],\"anonymous\":false},{\"type\":\"error\",\"name\":\"InvalidMessageProof\",\"inputs\":[]},{\"type\":\"error\",\"name\":\"InvalidMmrLeafProof\",\"inputs\":[]},{\"type\":\"error\",\"name\":\"NonceAlreadyDelivered\",\"inputs\":[]},{\"type\":\"error\",\"name\":\"UnsupportedCompactEncoding\",\"inputs\":[]}]",
	Bin: "0x60a0604052348015600e575f5ffd5b50604051610d16380380610d16833981016040819052602b91603b565b6001600160a01b03166080526066565b5f60208284031215604a575f5ffd5b81516001600160a01b0381168114605f575f5ffd5b9392505050565b608051610c926100845f395f818160be01526102bc0152610c925ff3fe608060405234801561000f575f5ffd5b506004361061007a575f3560e01c806378ef94a71161005857806378ef94a7146100f857806382646a581461012a578063a372e2e214610155578063fd10ebe514610168575f5ffd5b80630f6ea1421461007e5780631f0974a6146100a4578063776c81c3146100b9575b5f5ffd5b61009161008c366004610894565b61017a565b6040519081526020015b60405180910390f35b6100b76100b23660046108dd565b61025c565b005b6100e07f000000000000000000000000000000000000000000000000000000000000000081565b6040516001600160a01b03909116815260200161009b565b61011a610106366004610981565b60016020525f908152604090205460ff1681565b604051901515815260200161009b565b61013d6101383660046109ae565b6104df565b6040516001600160401b03909116815260200161009b565b610091610163366004610a1a565b610547565b5f5461013d906001600160401b031681565b5f6101f461018b6020840184610981565b5f65ff000000ff00600883811b91821664ff000000ff9185901c91821617601090811b67ff000000ff0000009390931666ff000000ff00009290921691909117901c17602081811b6bffffffffffffffff000000001691901c63ffffffff161760c01b92915050565b6102046040840160208501610a34565b60601b61021e6102176040860186610a5a565b9050610621565b61022b6040860186610a5a565b60405160200161023f959493929190610aa3565b604051602081830303815290604052805190602001209050919050565b60015f61026c6020870187610981565b6001600160401b0316815260208101919091526040015f205460ff16156102a657604051631096a5fb60e01b815260040160405180910390fd5b5f6102b084610547565b90506001600160a01b037f00000000000000000000000000000000000000000000000000000000000000001663a401662b826102ec8680610af8565b87602001356040518563ffffffff1660e01b81526004016103109493929190610b3d565b602060405180830381865afa15801561032b573d5f5f3e3d5ffd5b505050506040513d601f19601f8201168201806040525081019061034f9190610b81565b61036c57604051630eb01fc360e21b815260040160405180910390fd5b5f6103768661017a565b905061039a60c086013582853560208701356103956040890189610af8565b610657565b6103b75760405163c5501b6960e01b815260040160405180910390fd5b6001805f6103c860208a018a610981565b6001600160401b0316815260208082019290925260409081015f908120805460ff19169415159490941790935561040491908901908901610a34565b6001600160a01b031661041a6040890189610a5a565b604051610428929190610ba0565b5f604051808303815f865af19150503d805f8114610461576040519150601f19603f3d011682016040523d82523d5f602084013e610466565b606091505b5090915061047c90506040880160208901610a34565b6001600160a01b03166104926020890189610981565b6001600160401b03167fba554c1797f43598d59984cafb582089028d467bdc07c6902189a28123a4db23836040516104ce911515815260200190565b60405180910390a350505050505050565b5f80546001600160401b038082166001011667ffffffffffffffff199091168117909155604051339082907f182df4e02d6ffb7e98fd51a459949f7060b3655548cf8ea4d54d0c123cb28dfa906105399087908790610baf565b60405180910390a392915050565b5f61055e6105586020840184610bdd565b60f81b90565b6105986105716040850160208601610bfd565b600881811c62ff00ff1663ff00ff009290911b9190911617601081811c91901b1760e01b90565b60408401356105b061018b6080870160608801610981565b6105c361057160a0880160808901610bfd565b6040516001600160f81b031990951660208601526001600160e01b0319938416602186015260258501929092526001600160c01b031916604584015216604d82015260a0830135605182015260c0830135607182015260910161023f565b606063ffffffff82111561064857604051637404cccd60e11b815260040160405180910390fd5b61065182610682565b92915050565b5f83851061066657505f610678565b61067386868686866107d8565b871490505b9695505050505050565b6060603f8263ffffffff16116106bf57604051603f60fa1b60fa84901b1660208201526021015b6040516020818303038152906040529050919050565b613fff8263ffffffff161161071e576106fb6106e76403fffffffc600285901b166001610c20565b600881811b62ffff001691901c60ff161790565b6040516020016106a9919060f09190911b6001600160f01b031916815260020190565b633fffffff8263ffffffff16116107905761076d60028363ffffffff16901b60026107499190610c20565b600881811c62ff00ff1663ff00ff009290911b9190911617601081811c91901b1790565b6040516020016106a9919060e09190911b6001600160e01b031916815260040190565b604051600360f81b60208201526001600160e01b0319600884811c62ff00ff1663ff00ff009186901b9190911617601081811c91901b1760e01b1660218201526025016106a9565b5f85815b838110156108735786600116600114806107f857508587600101145b1561082f5761082885858381811061081257610812610c48565b90506020020135835f9182526020526040902090565b915061085d565b61085a8286868481811061084557610845610c48565b905060200201355f9182526020526040902090565b91505b600196871c965f19909601861c860195016107dc565b509695505050505050565b5f6060828403121561088e575f5ffd5b50919050565b5f602082840312156108a4575f5ffd5b81356001600160401b038111156108b9575f5ffd5b6108c58482850161087e565b949350505050565b5f60e0828403121561088e575f5ffd5b5f5f5f5f61014085870312156108f1575f5ffd5b84356001600160401b03811115610906575f5ffd5b6109128782880161087e565b94505061092286602087016108cd565b92506101008501356001600160401b0381111561093d575f5ffd5b85016040818803121561094e575f5ffd5b91506101208501356001600160401b03811115610969575f5ffd5b6109758782880161087e565b91505092959194509250565b5f60208284031215610991575f5ffd5b81356001600160401b03811681146109a7575f5ffd5b9392505050565b5f5f602083850312156109bf575f5ffd5b82356001600160401b038111156109d4575f5ffd5b8301601f810185136109e4575f5ffd5b80356001600160401b038111156109f9575f5ffd5b856020828401011115610a0a575f5ffd5b6020919091019590945092505050565b5f60e08284031215610a2a575f5ffd5b6109a783836108cd565b5f60208284031215610a44575f5ffd5b81356001600160a01b03811681146109a7575f5ffd5b5f5f8335601e19843603018112610a6f575f5ffd5b8301803591506001600160401b03821115610a88575f5ffd5b602001915036819003821315610a9c575f5ffd5b9250929050565b6001600160c01b0319861681526bffffffffffffffffffffffff198516600882015283515f908060208701601c85015e8083019050601c81015f815284868237505f9301601c01928352509095945050505050565b5f5f8335601e19843603018112610b0d575f5ffd5b8301803591506001600160401b03821115610b26575f5ffd5b6020019150600581901b3603821315610a9c575f5ffd5b84815260606020820181905281018390525f6001600160fb1b03841115610b62575f5ffd5b8360051b80866080850137604083019390935250016080019392505050565b5f60208284031215610b91575f5ffd5b815180151581146109a7575f5ffd5b818382375f9101908152919050565b60208152816020820152818360408301375f818301604090810191909152601f909201601f19160101919050565b5f60208284031215610bed575f5ffd5b813560ff811681146109a7575f5ffd5b5f60208284031215610c0d575f5ffd5b813563ffffffff811681146109a7575f5ffd5b63ffffffff818116838216019081111561065157634e487b7160e01b5f52601160045260245ffd5b634e487b7160e01b5f52603260045260245ffdfea264697066735822122064b33f694dee11fa7e44b8ae6da6cf1e158edffff13be217d92f29ec000dbe5f64736f6c63430008220033",
}

// GatewayABI is the input ABI used to generate the binding from.
// Deprecated: Use GatewayMetaData.ABI instead.
var GatewayABI = GatewayMetaData.ABI

// GatewayBin is the compiled bytecode used for deploying new contracts.
// Deprecated: Use GatewayMetaData.Bin instead.
var GatewayBin = GatewayMetaData.Bin

// DeployGateway deploys a new Ethereum contract, binding an instance of Gateway to it.
func DeployGateway(auth *bind.TransactOpts, backend bind.ContractBackend, _beefyClient common.Address) (common.Address, *types.Transaction, *Gateway, error) {
	parsed, err := GatewayMetaData.GetAbi()
	if err != nil {
		return common.Address{}, nil, nil, err
	}
	if parsed == nil {
		return common.Address{}, nil, nil, errors.New("GetABI returned nil")
	}

	address, tx, contract, err := bind.DeployContract(auth, *parsed, common.FromHex(GatewayBin), backend, _beefyClient)
	if err != nil {
		return common.Address{}, nil, nil, err
	}
	return address, tx, &Gateway{GatewayCaller: GatewayCaller{contract: contract}, GatewayTransactor: GatewayTransactor{contract: contract}, GatewayFilterer: GatewayFilterer{contract: contract}}, nil
}

// Gateway is an auto generated Go binding around an Ethereum contract.
type Gateway struct {
	GatewayCaller     // Read-only binding to the contract
	GatewayTransactor // Write-only binding to the contract
	GatewayFilterer   // Log filterer for contract events
}

// GatewayCaller is an auto generated read-only Go binding around an Ethereum contract.
type GatewayCaller struct {
	contract *bind.BoundContract // Generic contract wrapper for the low level calls
}

// GatewayTransactor is an auto generated write-only Go binding around an Ethereum contract.
type GatewayTransactor struct {
	contract *bind.BoundContract // Generic contract wrapper for the low level calls
}

// GatewayFilterer is an auto generated log filtering Go binding around an Ethereum contract events.
type GatewayFilterer struct {
	contract *bind.BoundContract // Generic contract wrapper for the low level calls
}

// GatewaySession is an auto generated Go binding around an Ethereum contract,
// with pre-set call and transact options.
type GatewaySession struct {
	Contract     *Gateway          // Generic contract binding to set the session for
	CallOpts     bind.CallOpts     // Call options to use throughout this session
	TransactOpts bind.TransactOpts // Transaction auth options to use throughout this session
}

// GatewayCallerSession is an auto generated read-only Go binding around an Ethereum contract,
// with pre-set call options.
type GatewayCallerSession struct {
	Contract *GatewayCaller // Generic contract caller binding to set the session for
	CallOpts bind.CallOpts  // Call options to use throughout this session
}

// GatewayTransactorSession is an auto generated write-only Go binding around an Ethereum contract,
// with pre-set transact options.
type GatewayTransactorSession struct {
	Contract     *GatewayTransactor // Generic contract transactor binding to set the session for
	TransactOpts bind.TransactOpts  // Transaction auth options to use throughout this session
}

// GatewayRaw is an auto generated low-level Go binding around an Ethereum contract.
type GatewayRaw struct {
	Contract *Gateway // Generic contract binding to access the raw methods on
}

// GatewayCallerRaw is an auto generated low-level read-only Go binding around an Ethereum contract.
type GatewayCallerRaw struct {
	Contract *GatewayCaller // Generic read-only contract binding to access the raw methods on
}

// GatewayTransactorRaw is an auto generated low-level write-only Go binding around an Ethereum contract.
type GatewayTransactorRaw struct {
	Contract *GatewayTransactor // Generic write-only contract binding to access the raw methods on
}

// NewGateway creates a new instance of Gateway, bound to a specific deployed contract.
func NewGateway(address common.Address, backend bind.ContractBackend) (*Gateway, error) {
	contract, err := bindGateway(address, backend, backend, backend)
	if err != nil {
		return nil, err
	}
	return &Gateway{GatewayCaller: GatewayCaller{contract: contract}, GatewayTransactor: GatewayTransactor{contract: contract}, GatewayFilterer: GatewayFilterer{contract: contract}}, nil
}

// NewGatewayCaller creates a new read-only instance of Gateway, bound to a specific deployed contract.
func NewGatewayCaller(address common.Address, caller bind.ContractCaller) (*GatewayCaller, error) {
	contract, err := bindGateway(address, caller, nil, nil)
	if err != nil {
		return nil, err
	}
	return &GatewayCaller{contract: contract}, nil
}

// NewGatewayTransactor creates a new write-only instance of Gateway, bound to a specific deployed contract.
func NewGatewayTransactor(address common.Address, transactor bind.ContractTransactor) (*GatewayTransactor, error) {
	contract, err := bindGateway(address, nil, transactor, nil)
	if err != nil {
		return nil, err
	}
	return &GatewayTransactor{contract: contract}, nil
}

// NewGatewayFilterer creates a new log filterer instance of Gateway, bound to a specific deployed contract.
func NewGatewayFilterer(address common.Address, filterer bind.ContractFilterer) (*GatewayFilterer, error) {
	contract, err := bindGateway(address, nil, nil, filterer)
	if err != nil {
		return nil, err
	}
	return &GatewayFilterer{contract: contract}, nil
}

// bindGateway binds a generic wrapper to an already deployed contract.
func bindGateway(address common.Address, caller bind.ContractCaller, transactor bind.ContractTransactor, filterer bind.ContractFilterer) (*bind.BoundContract, error) {
	parsed, err := GatewayMetaData.GetAbi()
	if err != nil {
		return nil, err
	}
	return bind.NewBoundContract(address, *parsed, caller, transactor, filterer), nil
}

// Call invokes the (constant) contract method with params as input values and
// sets the output to result. The result type might be a single field for simple
// returns, a slice of interfaces for anonymous returns and a struct for named
// returns.
func (_Gateway *GatewayRaw) Call(opts *bind.CallOpts, result *[]interface{}, method string, params ...interface{}) error {
	return _Gateway.Contract.GatewayCaller.contract.Call(opts, result, method, params...)
}

// Transfer initiates a plain transaction to move funds to the contract, calling
// its default method if one is available.
func (_Gateway *GatewayRaw) Transfer(opts *bind.TransactOpts) (*types.Transaction, error) {
	return _Gateway.Contract.GatewayTransactor.contract.Transfer(opts)
}

// Transact invokes the (paid) contract method with params as input values.
func (_Gateway *GatewayRaw) Transact(opts *bind.TransactOpts, method string, params ...interface{}) (*types.Transaction, error) {
	return _Gateway.Contract.GatewayTransactor.contract.Transact(opts, method, params...)
}

// Call invokes the (constant) contract method with params as input values and
// sets the output to result. The result type might be a single field for simple
// returns, a slice of interfaces for anonymous returns and a struct for named
// returns.
func (_Gateway *GatewayCallerRaw) Call(opts *bind.CallOpts, result *[]interface{}, method string, params ...interface{}) error {
	return _Gateway.Contract.contract.Call(opts, result, method, params...)
}

// Transfer initiates a plain transaction to move funds to the contract, calling
// its default method if one is available.
func (_Gateway *GatewayTransactorRaw) Transfer(opts *bind.TransactOpts) (*types.Transaction, error) {
	return _Gateway.Contract.contract.Transfer(opts)
}

// Transact invokes the (paid) contract method with params as input values.
func (_Gateway *GatewayTransactorRaw) Transact(opts *bind.TransactOpts, method string, params ...interface{}) (*types.Transaction, error) {
	return _Gateway.Contract.contract.Transact(opts, method, params...)
}

// BeefyClient is a free data retrieval call binding the contract method 0x776c81c3.
//
// Solidity: function beefyClient() view returns(address)
func (_Gateway *GatewayCaller) BeefyClient(opts *bind.CallOpts) (common.Address, error) {
	var out []interface{}
	err := _Gateway.contract.Call(opts, &out, "beefyClient")

	if err != nil {
		return *new(common.Address), err
	}

	out0 := *abi.ConvertType(out[0], new(common.Address)).(*common.Address)

	return out0, err

}

// BeefyClient is a free data retrieval call binding the contract method 0x776c81c3.
//
// Solidity: function beefyClient() view returns(address)
func (_Gateway *GatewaySession) BeefyClient() (common.Address, error) {
	return _Gateway.Contract.BeefyClient(&_Gateway.CallOpts)
}

// BeefyClient is a free data retrieval call binding the contract method 0x776c81c3.
//
// Solidity: function beefyClient() view returns(address)
func (_Gateway *GatewayCallerSession) BeefyClient() (common.Address, error) {
	return _Gateway.Contract.BeefyClient(&_Gateway.CallOpts)
}

// HashMessageLeaf is a free data retrieval call binding the contract method 0x0f6ea142.
//
// Solidity: function hashMessageLeaf((uint64,address,bytes) m) pure returns(bytes32)
func (_Gateway *GatewayCaller) HashMessageLeaf(opts *bind.CallOpts, m OutboundMessage) ([32]byte, error) {
	var out []interface{}
	err := _Gateway.contract.Call(opts, &out, "hashMessageLeaf", m)

	if err != nil {
		return *new([32]byte), err
	}

	out0 := *abi.ConvertType(out[0], new([32]byte)).(*[32]byte)

	return out0, err

}

// HashMessageLeaf is a free data retrieval call binding the contract method 0x0f6ea142.
//
// Solidity: function hashMessageLeaf((uint64,address,bytes) m) pure returns(bytes32)
func (_Gateway *GatewaySession) HashMessageLeaf(m OutboundMessage) ([32]byte, error) {
	return _Gateway.Contract.HashMessageLeaf(&_Gateway.CallOpts, m)
}

// HashMessageLeaf is a free data retrieval call binding the contract method 0x0f6ea142.
//
// Solidity: function hashMessageLeaf((uint64,address,bytes) m) pure returns(bytes32)
func (_Gateway *GatewayCallerSession) HashMessageLeaf(m OutboundMessage) ([32]byte, error) {
	return _Gateway.Contract.HashMessageLeaf(&_Gateway.CallOpts, m)
}

// HashMmrLeaf is a free data retrieval call binding the contract method 0xa372e2e2.
//
// Solidity: function hashMmrLeaf((uint8,uint32,bytes32,uint64,uint32,bytes32,bytes32) leaf) pure returns(bytes32)
func (_Gateway *GatewayCaller) HashMmrLeaf(opts *bind.CallOpts, leaf GatewayMmrLeaf) ([32]byte, error) {
	var out []interface{}
	err := _Gateway.contract.Call(opts, &out, "hashMmrLeaf", leaf)

	if err != nil {
		return *new([32]byte), err
	}

	out0 := *abi.ConvertType(out[0], new([32]byte)).(*[32]byte)

	return out0, err

}

// HashMmrLeaf is a free data retrieval call binding the contract method 0xa372e2e2.
//
// Solidity: function hashMmrLeaf((uint8,uint32,bytes32,uint64,uint32,bytes32,bytes32) leaf) pure returns(bytes32)
func (_Gateway *GatewaySession) HashMmrLeaf(leaf GatewayMmrLeaf) ([32]byte, error) {
	return _Gateway.Contract.HashMmrLeaf(&_Gateway.CallOpts, leaf)
}

// HashMmrLeaf is a free data retrieval call binding the contract method 0xa372e2e2.
//
// Solidity: function hashMmrLeaf((uint8,uint32,bytes32,uint64,uint32,bytes32,bytes32) leaf) pure returns(bytes32)
func (_Gateway *GatewayCallerSession) HashMmrLeaf(leaf GatewayMmrLeaf) ([32]byte, error) {
	return _Gateway.Contract.HashMmrLeaf(&_Gateway.CallOpts, leaf)
}

// InboundDelivered is a free data retrieval call binding the contract method 0x78ef94a7.
//
// Solidity: function inboundDelivered(uint64 nonce) view returns(bool)
func (_Gateway *GatewayCaller) InboundDelivered(opts *bind.CallOpts, nonce uint64) (bool, error) {
	var out []interface{}
	err := _Gateway.contract.Call(opts, &out, "inboundDelivered", nonce)

	if err != nil {
		return *new(bool), err
	}

	out0 := *abi.ConvertType(out[0], new(bool)).(*bool)

	return out0, err

}

// InboundDelivered is a free data retrieval call binding the contract method 0x78ef94a7.
//
// Solidity: function inboundDelivered(uint64 nonce) view returns(bool)
func (_Gateway *GatewaySession) InboundDelivered(nonce uint64) (bool, error) {
	return _Gateway.Contract.InboundDelivered(&_Gateway.CallOpts, nonce)
}

// InboundDelivered is a free data retrieval call binding the contract method 0x78ef94a7.
//
// Solidity: function inboundDelivered(uint64 nonce) view returns(bool)
func (_Gateway *GatewayCallerSession) InboundDelivered(nonce uint64) (bool, error) {
	return _Gateway.Contract.InboundDelivered(&_Gateway.CallOpts, nonce)
}

// OutboundNonce is a free data retrieval call binding the contract method 0xfd10ebe5.
//
// Solidity: function outboundNonce() view returns(uint64)
func (_Gateway *GatewayCaller) OutboundNonce(opts *bind.CallOpts) (uint64, error) {
	var out []interface{}
	err := _Gateway.contract.Call(opts, &out, "outboundNonce")

	if err != nil {
		return *new(uint64), err
	}

	out0 := *abi.ConvertType(out[0], new(uint64)).(*uint64)

	return out0, err

}

// OutboundNonce is a free data retrieval call binding the contract method 0xfd10ebe5.
//
// Solidity: function outboundNonce() view returns(uint64)
func (_Gateway *GatewaySession) OutboundNonce() (uint64, error) {
	return _Gateway.Contract.OutboundNonce(&_Gateway.CallOpts)
}

// OutboundNonce is a free data retrieval call binding the contract method 0xfd10ebe5.
//
// Solidity: function outboundNonce() view returns(uint64)
func (_Gateway *GatewayCallerSession) OutboundNonce() (uint64, error) {
	return _Gateway.Contract.OutboundNonce(&_Gateway.CallOpts)
}

// SendMessage is a paid mutator transaction binding the contract method 0x82646a58.
//
// Solidity: function sendMessage(bytes payload) returns(uint64 nonce)
func (_Gateway *GatewayTransactor) SendMessage(opts *bind.TransactOpts, payload []byte) (*types.Transaction, error) {
	return _Gateway.contract.Transact(opts, "sendMessage", payload)
}

// SendMessage is a paid mutator transaction binding the contract method 0x82646a58.
//
// Solidity: function sendMessage(bytes payload) returns(uint64 nonce)
func (_Gateway *GatewaySession) SendMessage(payload []byte) (*types.Transaction, error) {
	return _Gateway.Contract.SendMessage(&_Gateway.TransactOpts, payload)
}

// SendMessage is a paid mutator transaction binding the contract method 0x82646a58.
//
// Solidity: function sendMessage(bytes payload) returns(uint64 nonce)
func (_Gateway *GatewayTransactorSession) SendMessage(payload []byte) (*types.Transaction, error) {
	return _Gateway.Contract.SendMessage(&_Gateway.TransactOpts, payload)
}

// SubmitInbound is a paid mutator transaction binding the contract method 0x1f0974a6.
//
// Solidity: function submitInbound((uint64,address,bytes) message, (uint8,uint32,bytes32,uint64,uint32,bytes32,bytes32) leaf, (bytes32[],uint256) leafProof, (uint256,uint256,bytes32[]) msgProof) returns()
func (_Gateway *GatewayTransactor) SubmitInbound(opts *bind.TransactOpts, message OutboundMessage, leaf GatewayMmrLeaf, leafProof GatewayMmrLeafProof, msgProof GatewayMessageProof) (*types.Transaction, error) {
	return _Gateway.contract.Transact(opts, "submitInbound", message, leaf, leafProof, msgProof)
}

// SubmitInbound is a paid mutator transaction binding the contract method 0x1f0974a6.
//
// Solidity: function submitInbound((uint64,address,bytes) message, (uint8,uint32,bytes32,uint64,uint32,bytes32,bytes32) leaf, (bytes32[],uint256) leafProof, (uint256,uint256,bytes32[]) msgProof) returns()
func (_Gateway *GatewaySession) SubmitInbound(message OutboundMessage, leaf GatewayMmrLeaf, leafProof GatewayMmrLeafProof, msgProof GatewayMessageProof) (*types.Transaction, error) {
	return _Gateway.Contract.SubmitInbound(&_Gateway.TransactOpts, message, leaf, leafProof, msgProof)
}

// SubmitInbound is a paid mutator transaction binding the contract method 0x1f0974a6.
//
// Solidity: function submitInbound((uint64,address,bytes) message, (uint8,uint32,bytes32,uint64,uint32,bytes32,bytes32) leaf, (bytes32[],uint256) leafProof, (uint256,uint256,bytes32[]) msgProof) returns()
func (_Gateway *GatewayTransactorSession) SubmitInbound(message OutboundMessage, leaf GatewayMmrLeaf, leafProof GatewayMmrLeafProof, msgProof GatewayMessageProof) (*types.Transaction, error) {
	return _Gateway.Contract.SubmitInbound(&_Gateway.TransactOpts, message, leaf, leafProof, msgProof)
}

// GatewayInboundMessageDispatchedIterator is returned from FilterInboundMessageDispatched and is used to iterate over the raw logs and unpacked data for InboundMessageDispatched events raised by the Gateway contract.
type GatewayInboundMessageDispatchedIterator struct {
	Event *GatewayInboundMessageDispatched // Event containing the contract specifics and raw log

	contract *bind.BoundContract // Generic contract to use for unpacking event data
	event    string              // Event name to use for unpacking event data

	logs chan types.Log        // Log channel receiving the found contract events
	sub  ethereum.Subscription // Subscription for errors, completion and termination
	done bool                  // Whether the subscription completed delivering logs
	fail error                 // Occurred error to stop iteration
}

// Next advances the iterator to the subsequent event, returning whether there
// are any more events found. In case of a retrieval or parsing error, false is
// returned and Error() can be queried for the exact failure.
func (it *GatewayInboundMessageDispatchedIterator) Next() bool {
	// If the iterator failed, stop iterating
	if it.fail != nil {
		return false
	}
	// If the iterator completed, deliver directly whatever's available
	if it.done {
		select {
		case log := <-it.logs:
			it.Event = new(GatewayInboundMessageDispatched)
			if err := it.contract.UnpackLog(it.Event, it.event, log); err != nil {
				it.fail = err
				return false
			}
			it.Event.Raw = log
			return true

		default:
			return false
		}
	}
	// Iterator still in progress, wait for either a data or an error event
	select {
	case log := <-it.logs:
		it.Event = new(GatewayInboundMessageDispatched)
		if err := it.contract.UnpackLog(it.Event, it.event, log); err != nil {
			it.fail = err
			return false
		}
		it.Event.Raw = log
		return true

	case err := <-it.sub.Err():
		it.done = true
		it.fail = err
		return it.Next()
	}
}

// Error returns any retrieval or parsing error occurred during filtering.
func (it *GatewayInboundMessageDispatchedIterator) Error() error {
	return it.fail
}

// Close terminates the iteration process, releasing any pending underlying
// resources.
func (it *GatewayInboundMessageDispatchedIterator) Close() error {
	it.sub.Unsubscribe()
	return nil
}

// GatewayInboundMessageDispatched represents a InboundMessageDispatched event raised by the Gateway contract.
type GatewayInboundMessageDispatched struct {
	Nonce       uint64
	Destination common.Address
	Success     bool
	Raw         types.Log // Blockchain specific contextual infos
}

// FilterInboundMessageDispatched is a free log retrieval operation binding the contract event 0xba554c1797f43598d59984cafb582089028d467bdc07c6902189a28123a4db23.
//
// Solidity: event InboundMessageDispatched(uint64 indexed nonce, address indexed destination, bool success)
func (_Gateway *GatewayFilterer) FilterInboundMessageDispatched(opts *bind.FilterOpts, nonce []uint64, destination []common.Address) (*GatewayInboundMessageDispatchedIterator, error) {

	var nonceRule []interface{}
	for _, nonceItem := range nonce {
		nonceRule = append(nonceRule, nonceItem)
	}
	var destinationRule []interface{}
	for _, destinationItem := range destination {
		destinationRule = append(destinationRule, destinationItem)
	}

	logs, sub, err := _Gateway.contract.FilterLogs(opts, "InboundMessageDispatched", nonceRule, destinationRule)
	if err != nil {
		return nil, err
	}
	return &GatewayInboundMessageDispatchedIterator{contract: _Gateway.contract, event: "InboundMessageDispatched", logs: logs, sub: sub}, nil
}

// WatchInboundMessageDispatched is a free log subscription operation binding the contract event 0xba554c1797f43598d59984cafb582089028d467bdc07c6902189a28123a4db23.
//
// Solidity: event InboundMessageDispatched(uint64 indexed nonce, address indexed destination, bool success)
func (_Gateway *GatewayFilterer) WatchInboundMessageDispatched(opts *bind.WatchOpts, sink chan<- *GatewayInboundMessageDispatched, nonce []uint64, destination []common.Address) (event.Subscription, error) {

	var nonceRule []interface{}
	for _, nonceItem := range nonce {
		nonceRule = append(nonceRule, nonceItem)
	}
	var destinationRule []interface{}
	for _, destinationItem := range destination {
		destinationRule = append(destinationRule, destinationItem)
	}

	logs, sub, err := _Gateway.contract.WatchLogs(opts, "InboundMessageDispatched", nonceRule, destinationRule)
	if err != nil {
		return nil, err
	}
	return event.NewSubscription(func(quit <-chan struct{}) error {
		defer sub.Unsubscribe()
		for {
			select {
			case log := <-logs:
				// New log arrived, parse the event and forward to the user
				event := new(GatewayInboundMessageDispatched)
				if err := _Gateway.contract.UnpackLog(event, "InboundMessageDispatched", log); err != nil {
					return err
				}
				event.Raw = log

				select {
				case sink <- event:
				case err := <-sub.Err():
					return err
				case <-quit:
					return nil
				}
			case err := <-sub.Err():
				return err
			case <-quit:
				return nil
			}
		}
	}), nil
}

// ParseInboundMessageDispatched is a log parse operation binding the contract event 0xba554c1797f43598d59984cafb582089028d467bdc07c6902189a28123a4db23.
//
// Solidity: event InboundMessageDispatched(uint64 indexed nonce, address indexed destination, bool success)
func (_Gateway *GatewayFilterer) ParseInboundMessageDispatched(log types.Log) (*GatewayInboundMessageDispatched, error) {
	event := new(GatewayInboundMessageDispatched)
	if err := _Gateway.contract.UnpackLog(event, "InboundMessageDispatched", log); err != nil {
		return nil, err
	}
	event.Raw = log
	return event, nil
}

// GatewayOutboundMessageAcceptedIterator is returned from FilterOutboundMessageAccepted and is used to iterate over the raw logs and unpacked data for OutboundMessageAccepted events raised by the Gateway contract.
type GatewayOutboundMessageAcceptedIterator struct {
	Event *GatewayOutboundMessageAccepted // Event containing the contract specifics and raw log

	contract *bind.BoundContract // Generic contract to use for unpacking event data
	event    string              // Event name to use for unpacking event data

	logs chan types.Log        // Log channel receiving the found contract events
	sub  ethereum.Subscription // Subscription for errors, completion and termination
	done bool                  // Whether the subscription completed delivering logs
	fail error                 // Occurred error to stop iteration
}

// Next advances the iterator to the subsequent event, returning whether there
// are any more events found. In case of a retrieval or parsing error, false is
// returned and Error() can be queried for the exact failure.
func (it *GatewayOutboundMessageAcceptedIterator) Next() bool {
	// If the iterator failed, stop iterating
	if it.fail != nil {
		return false
	}
	// If the iterator completed, deliver directly whatever's available
	if it.done {
		select {
		case log := <-it.logs:
			it.Event = new(GatewayOutboundMessageAccepted)
			if err := it.contract.UnpackLog(it.Event, it.event, log); err != nil {
				it.fail = err
				return false
			}
			it.Event.Raw = log
			return true

		default:
			return false
		}
	}
	// Iterator still in progress, wait for either a data or an error event
	select {
	case log := <-it.logs:
		it.Event = new(GatewayOutboundMessageAccepted)
		if err := it.contract.UnpackLog(it.Event, it.event, log); err != nil {
			it.fail = err
			return false
		}
		it.Event.Raw = log
		return true

	case err := <-it.sub.Err():
		it.done = true
		it.fail = err
		return it.Next()
	}
}

// Error returns any retrieval or parsing error occurred during filtering.
func (it *GatewayOutboundMessageAcceptedIterator) Error() error {
	return it.fail
}

// Close terminates the iteration process, releasing any pending underlying
// resources.
func (it *GatewayOutboundMessageAcceptedIterator) Close() error {
	it.sub.Unsubscribe()
	return nil
}

// GatewayOutboundMessageAccepted represents a OutboundMessageAccepted event raised by the Gateway contract.
type GatewayOutboundMessageAccepted struct {
	Nonce   uint64
	Origin  common.Address
	Payload []byte
	Raw     types.Log // Blockchain specific contextual infos
}

// FilterOutboundMessageAccepted is a free log retrieval operation binding the contract event 0x182df4e02d6ffb7e98fd51a459949f7060b3655548cf8ea4d54d0c123cb28dfa.
//
// Solidity: event OutboundMessageAccepted(uint64 indexed nonce, address indexed origin, bytes payload)
func (_Gateway *GatewayFilterer) FilterOutboundMessageAccepted(opts *bind.FilterOpts, nonce []uint64, origin []common.Address) (*GatewayOutboundMessageAcceptedIterator, error) {

	var nonceRule []interface{}
	for _, nonceItem := range nonce {
		nonceRule = append(nonceRule, nonceItem)
	}
	var originRule []interface{}
	for _, originItem := range origin {
		originRule = append(originRule, originItem)
	}

	logs, sub, err := _Gateway.contract.FilterLogs(opts, "OutboundMessageAccepted", nonceRule, originRule)
	if err != nil {
		return nil, err
	}
	return &GatewayOutboundMessageAcceptedIterator{contract: _Gateway.contract, event: "OutboundMessageAccepted", logs: logs, sub: sub}, nil
}

// WatchOutboundMessageAccepted is a free log subscription operation binding the contract event 0x182df4e02d6ffb7e98fd51a459949f7060b3655548cf8ea4d54d0c123cb28dfa.
//
// Solidity: event OutboundMessageAccepted(uint64 indexed nonce, address indexed origin, bytes payload)
func (_Gateway *GatewayFilterer) WatchOutboundMessageAccepted(opts *bind.WatchOpts, sink chan<- *GatewayOutboundMessageAccepted, nonce []uint64, origin []common.Address) (event.Subscription, error) {

	var nonceRule []interface{}
	for _, nonceItem := range nonce {
		nonceRule = append(nonceRule, nonceItem)
	}
	var originRule []interface{}
	for _, originItem := range origin {
		originRule = append(originRule, originItem)
	}

	logs, sub, err := _Gateway.contract.WatchLogs(opts, "OutboundMessageAccepted", nonceRule, originRule)
	if err != nil {
		return nil, err
	}
	return event.NewSubscription(func(quit <-chan struct{}) error {
		defer sub.Unsubscribe()
		for {
			select {
			case log := <-logs:
				// New log arrived, parse the event and forward to the user
				event := new(GatewayOutboundMessageAccepted)
				if err := _Gateway.contract.UnpackLog(event, "OutboundMessageAccepted", log); err != nil {
					return err
				}
				event.Raw = log

				select {
				case sink <- event:
				case err := <-sub.Err():
					return err
				case <-quit:
					return nil
				}
			case err := <-sub.Err():
				return err
			case <-quit:
				return nil
			}
		}
	}), nil
}

// ParseOutboundMessageAccepted is a log parse operation binding the contract event 0x182df4e02d6ffb7e98fd51a459949f7060b3655548cf8ea4d54d0c123cb28dfa.
//
// Solidity: event OutboundMessageAccepted(uint64 indexed nonce, address indexed origin, bytes payload)
func (_Gateway *GatewayFilterer) ParseOutboundMessageAccepted(log types.Log) (*GatewayOutboundMessageAccepted, error) {
	event := new(GatewayOutboundMessageAccepted)
	if err := _Gateway.contract.UnpackLog(event, "OutboundMessageAccepted", log); err != nil {
		return nil, err
	}
	event.Raw = log
	return event, nil
}
