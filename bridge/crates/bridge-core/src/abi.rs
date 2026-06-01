//! On-chain Gate bindings (alloy `sol!`). Only compiled with feature `abi`.
//!
//! The event/function signatures here MUST match `contracts/src/Gate.sol`.

use alloy::sol;

sol! {
    /// To-side execution payload; abi.encode'd into the `autoParams` bytes.
    struct AutoParamsTo {
        uint256 executionFee;
        uint256 flags;
        bytes fallbackAddress;
        bytes data;
    }

    #[sol(rpc)]
    contract Gate {
        event Sent(
            bytes32 indexed submissionId,
            bytes32 indexed debridgeId,
            uint256 amount,
            uint256 chainIdFrom,
            uint256 chainIdTo,
            bytes receiver,
            uint256 nonce,
            bytes autoParams,
            bytes nativeSender
        );

        event Claimed(
            bytes32 indexed submissionId,
            bytes32 indexed debridgeId,
            address indexed receiver,
            uint256 amount
        );

        function send(
            address token,
            uint256 amount,
            uint256 chainIdTo,
            bytes receiver,
            bytes autoParams
        ) external returns (bytes32 submissionId);

        function claim(
            bytes32 debridgeId,
            uint256 amount,
            uint256 chainIdFrom,
            uint256 nonce,
            bytes receiver,
            bytes autoParams,
            bytes nativeSender,
            bytes[] signatures
        ) external returns (bytes32 submissionId);

        function executed(bytes32 submissionId) external view returns (bool);
        function threshold() external view returns (uint256);
        function nonceTo(uint256 chainIdTo) external view returns (uint256);
        function setLocalToken(bytes32 debridgeId, address localToken) external;
        function setValidator(address v, bool active) external;
    }

    #[sol(rpc)]
    contract IERC20Mintable {
        function approve(address spender, uint256 amount) external returns (bool);
        function balanceOf(address account) external view returns (uint256);
        function mint(address to, uint256 amount) external;
    }
}
