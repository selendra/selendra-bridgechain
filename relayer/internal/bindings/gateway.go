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
}

// GatewayABI is the input ABI used to generate the binding from.
// Deprecated: Use GatewayMetaData.ABI instead.
var GatewayABI = GatewayMetaData.ABI

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
